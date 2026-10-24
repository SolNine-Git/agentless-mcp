//! The labeling file in the excel dialect: the row writer, and a reader that reads a file as the
//! reference does. That reader decodes the file in 8192-byte chunks and keeps every line ending,
//! so a decode error carries the same position and comes after the same rows.

use crate::report::log::decoded;

const CHUNK: usize = 8192;
pub(crate) const FIELD_LIMIT: usize = 131_072;
const QUOTE: char = '"';
const DELIMITER: char = ',';

// One record: a field is quoted when it holds the delimiter, a quote, CR or LF, and a quote in it
// is doubled. The record ends with CRLF.
pub(crate) fn excel_row(fields: &[String]) -> String {
    let mut row = String::new();
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            row.push(DELIMITER);
        }
        if field.contains([DELIMITER, QUOTE, '\r', '\n']) {
            row.push(QUOTE);
            row.push_str(&field.replace(QUOTE, "\"\""));
            row.push(QUOTE);
        } else {
            row.push_str(field);
        }
    }
    row.push_str("\r\n");
    row
}

// The lines of a file read as text with newline="": each line keeps its ending, and a CR at the
// end of a chunk's text waits for the next chunk, so a CRLF is never split.
struct Lines<'b> {
    bytes: &'b [u8],
    offset: usize,
    pending: Vec<u8>,
    text: String,
    held_cr: bool,
    ended: bool,
    failed: bool,
}

impl Iterator for Lines<'_> {
    type Item = Result<String, String>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(end) = line_end(&self.text) {
                return Some(Ok(self.text.drain(..end).collect()));
            }
            if self.failed {
                return None;
            }
            if self.ended {
                let rest = std::mem::take(&mut self.text);
                return (!rest.is_empty()).then_some(Ok(rest));
            }
            if let Err(error) = self.decode_chunk() {
                self.failed = true;
                return Some(Err(error));
            }
        }
    }
}

impl Lines<'_> {
    fn decode_chunk(&mut self) -> Result<(), String> {
        let end = (self.offset + CHUNK).min(self.bytes.len());
        let last = end == self.offset;
        let mut data = std::mem::take(&mut self.pending);
        data.extend_from_slice(&self.bytes[self.offset..end]);
        self.offset = end;
        let mut text = incremental(&mut data, last)?;
        self.pending = data;
        if self.held_cr && (last || !text.is_empty()) {
            text.insert(0, '\r');
            self.held_cr = false;
        }
        if !last && text.ends_with('\r') {
            text.pop();
            self.held_cr = true;
        }
        self.text.push_str(&text);
        self.ended = last;
        Ok(())
    }
}

// The text of `data` up to a sequence that the next chunk can complete, which stays in `data`. At
// the last chunk every sequence must be complete.
fn incremental(data: &mut Vec<u8>, last: bool) -> Result<String, String> {
    match std::str::from_utf8(data) {
        Ok(text) => {
            let text = text.to_owned();
            data.clear();
            Ok(text)
        }
        Err(error) if error.error_len().is_none() && !last => {
            let rest = data.split_off(error.valid_up_to());
            let text = std::mem::replace(data, rest);
            decoded(text)
        }
        Err(_) => decoded(std::mem::take(data)),
    }
}

// The end of the first line in `text`: after LF, after CRLF, or after a CR that no LF follows.
fn line_end(text: &str) -> Option<usize> {
    let at = text.find(['\r', '\n'])?;
    if text[at..].starts_with("\r\n") {
        return Some(at + 2);
    }
    Some(at + 1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    StartRecord,
    StartField,
    InField,
    InQuotedField,
    QuoteInQuotedField,
    EatCrnl,
}

// One record being parsed; `None` stands for the end of a line.
struct Parse {
    fields: Vec<String>,
    field: String,
    length: usize,
    state: State,
}

impl Parse {
    fn new() -> Self {
        Self {
            fields: Vec::new(),
            field: String::new(),
            length: 0,
            state: State::StartRecord,
        }
    }

    fn add(&mut self, c: char) -> Result<(), String> {
        if self.length >= FIELD_LIMIT {
            return Err(format!("field larger than field limit ({FIELD_LIMIT})"));
        }
        self.field.push(c);
        self.length += 1;
        Ok(())
    }

    fn save(&mut self) {
        self.fields.push(std::mem::take(&mut self.field));
        self.length = 0;
    }

    // Saves the field at a line end, then waits for a record or for the rest of the line ending.
    fn end_record(&mut self, c: Option<char>) {
        self.save();
        self.state = if c.is_none() {
            State::StartRecord
        } else {
            State::EatCrnl
        };
    }

    // Each character of one line, then the line's end.
    fn line(&mut self, line: &str) -> Result<(), String> {
        line.chars()
            .map(Some)
            .chain([None])
            .try_for_each(|c| self.step(c))
    }

    fn step(&mut self, c: Option<char>) -> Result<(), String> {
        match self.state {
            State::StartRecord => self.start_record(c),
            State::StartField => self.start_field(c),
            State::InField => self.in_field(c),
            State::InQuotedField => self.in_quoted(c),
            State::QuoteInQuotedField => self.quote_in_quoted(c),
            State::EatCrnl => eat_crnl(&mut self.state, c),
        }
    }

    fn start_record(&mut self, c: Option<char>) -> Result<(), String> {
        match c {
            None => Ok(()),
            Some('\n' | '\r') => {
                self.state = State::EatCrnl;
                Ok(())
            }
            Some(_) => {
                self.state = State::StartField;
                self.start_field(c)
            }
        }
    }

    fn start_field(&mut self, c: Option<char>) -> Result<(), String> {
        match c {
            None | Some('\n' | '\r') => self.end_record(c),
            Some(QUOTE) => self.state = State::InQuotedField,
            Some(DELIMITER) => self.save(),
            Some(c) => {
                self.add(c)?;
                self.state = State::InField;
            }
        }
        Ok(())
    }

    fn in_field(&mut self, c: Option<char>) -> Result<(), String> {
        match c {
            None | Some('\n' | '\r') => self.end_record(c),
            Some(DELIMITER) => {
                self.save();
                self.state = State::StartField;
            }
            Some(c) => self.add(c)?,
        }
        Ok(())
    }

    fn in_quoted(&mut self, c: Option<char>) -> Result<(), String> {
        match c {
            None => {}
            Some(QUOTE) => self.state = State::QuoteInQuotedField,
            Some(c) => self.add(c)?,
        }
        Ok(())
    }

    // A quote in a quoted field: a doubled quote, the field's end, or (not strict) more text.
    fn quote_in_quoted(&mut self, c: Option<char>) -> Result<(), String> {
        match c {
            Some(QUOTE) => {
                self.add(QUOTE)?;
                self.state = State::InQuotedField;
            }
            Some(DELIMITER) => {
                self.save();
                self.state = State::StartField;
            }
            None | Some('\n' | '\r') => self.end_record(c),
            Some(c) => {
                self.add(c)?;
                self.state = State::InField;
            }
        }
        Ok(())
    }
}

fn eat_crnl(state: &mut State, c: Option<char>) -> Result<(), String> {
    match c {
        Some('\n' | '\r') => Ok(()),
        None => {
            *state = State::StartRecord;
            Ok(())
        }
        Some(_) => Err(
            "new-line character seen in unquoted field - do you need to open the file \
                        with newline=''?"
                .to_owned(),
        ),
    }
}

// The records of a file, and the count of lines read so far.
struct Reader<'b> {
    lines: Lines<'b>,
    line_num: usize,
}

impl Reader<'_> {
    fn next_record(&mut self) -> Option<Result<Vec<String>, String>> {
        let mut parse = Parse::new();
        loop {
            let line = match self.lines.next() {
                None => return at_end(parse),
                Some(Err(error)) => return Some(Err(error)),
                Some(Ok(line)) => line,
            };
            self.line_num += 1;
            if let Err(error) = parse.line(&line) {
                return Some(Err(error));
            }
            if parse.state == State::StartRecord {
                return Some(Ok(parse.fields));
            }
        }
    }

    // The next record that is not blank.
    fn next_filled(&mut self) -> Option<Result<Vec<String>, String>> {
        loop {
            let record = self.next_record()?;
            if !matches!(&record, Ok(fields) if fields.is_empty()) {
                return Some(record);
            }
        }
    }
}

// The input ended inside a record: a quoted field or a field with text is saved, else no record.
fn at_end(mut parse: Parse) -> Option<Result<Vec<String>, String>> {
    if parse.length == 0 && parse.state != State::InQuotedField {
        return None;
    }
    parse.save();
    Some(Ok(parse.fields))
}

// One value of a row read by name: the text, a column the row lacks, or the columns past the header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Cell {
    Text(String),
    Missing,
    Rest(Vec<String>),
}

// A row by name, in first-seen order; `None` names the columns past the header.
pub(crate) type Row = Vec<(Option<String>, Cell)>;

// The rows of a file by the names of its first record. Blank records are skipped, and `line_num`
// counts the lines read through the last row.
pub(crate) struct DictReader<'b> {
    reader: Reader<'b>,
    fieldnames: Option<Vec<String>>,
    read_header: bool,
}

impl<'b> DictReader<'b> {
    pub(crate) fn new(bytes: &'b [u8]) -> Self {
        let lines = Lines {
            bytes,
            offset: 0,
            pending: Vec::new(),
            text: String::new(),
            held_cr: false,
            ended: false,
            failed: false,
        };
        Self {
            reader: Reader { lines, line_num: 0 },
            fieldnames: None,
            read_header: false,
        }
    }

    pub(crate) fn line_num(&self) -> usize {
        self.reader.line_num
    }

    // The first record, or `None` for a file without one.
    pub(crate) fn fieldnames(&mut self) -> Result<Option<&[String]>, String> {
        if !self.read_header {
            self.read_header = true;
            self.fieldnames = self.reader.next_record().transpose()?;
        }
        Ok(self.fieldnames.as_deref())
    }

    pub(crate) fn next_row(&mut self) -> Option<Result<Row, String>> {
        if let Err(error) = self.fieldnames() {
            return Some(Err(error));
        }
        let record = self.reader.next_filled()?;
        // A file without a first record has no later one.
        let names = self.fieldnames.as_deref()?;
        Some(record.map(|record| by_name(names, record)))
    }
}

// A later column of a repeated name replaces the value, and keeps the first position.
fn by_name(names: &[String], mut record: Vec<String>) -> Row {
    let count = record.len();
    let rest = record.split_off(count.min(names.len()));
    let mut row: Row = Vec::new();
    for (name, value) in names.iter().zip(record) {
        set(&mut row, Some(name.clone()), Cell::Text(value));
    }
    if !rest.is_empty() {
        set(&mut row, None, Cell::Rest(rest));
    }
    for name in names.iter().skip(count) {
        set(&mut row, Some(name.clone()), Cell::Missing);
    }
    row
}

fn set(row: &mut Row, name: Option<String>, cell: Cell) {
    match row.iter_mut().find(|(key, _)| *key == name) {
        Some((_, slot)) => *slot = cell,
        None => row.push((name, cell)),
    }
}
