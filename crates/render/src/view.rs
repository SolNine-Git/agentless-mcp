//! The two forms of a read answer: the text under its receipt, and the JSON document that starts
//! with its receipt.

use serde::Serialize;

use crate::count::Ceiling;
use crate::envelope::{Listing, wrap};
use crate::{Error, Receipt};

/// What a service left out of an answer: `shown` of `total` items of one unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Truncation {
    /// The items the answer shows.
    pub shown: usize,
    /// The items the service considered.
    pub total: usize,
    /// The items' noun, for example "symbols".
    pub unit: &'static str,
}

/// A read result that render prints as text and as JSON.
pub trait View {
    /// Returns the text body, the JSON payload and the truncation of this result.
    fn rendered(&self) -> Rendered<'_>;
}

/// The text body, the JSON payload and the truncation of one view. Only render builds it.
pub struct Rendered<'a> {
    body: String,
    truncation: Option<Truncation>,
    document: Box<dyn Document + 'a>,
}

// The envelope function that writes one payload's JSON document: `envelope::wrap_json` for a
// payload with one list to trim at the ceiling, `envelope::wrap_json_whole` for any other.
pub(crate) type Writer<P> =
    fn(&Receipt<'_>, &WithTruncation<P>, &Ceiling<'_>) -> Result<String, Error>;

impl<'a> Rendered<'a> {
    // The view's text body, its JSON payload with the writer of its document, and its cut.
    pub(crate) fn new<P: 'a>(
        body: String,
        payload: P,
        truncation: Option<Truncation>,
        write: Writer<P>,
    ) -> Rendered<'a> {
        let content = WithTruncation::new(payload, truncation);
        Rendered {
            body,
            truncation,
            document: Box::new(JsonDocument { content, write }),
        }
    }
}

/// Returns `view` as text under its receipt, with the service's note, cut at the token ceiling.
#[must_use]
pub fn text(receipt: &Receipt<'_>, view: &impl View) -> String {
    let rendered = view.rendered();
    let ceiling = receipt.ceiling();
    wrap(
        receipt,
        &rendered.body,
        &ceiling,
        rendered.truncation.as_ref(),
    )
}

/// Returns `view` as a JSON document that starts with its receipt, trimmed at the token ceiling.
///
/// # Errors
///
/// Returns the serializer's error for a value it cannot write.
pub fn json(receipt: &Receipt<'_>, view: &impl View) -> Result<String, Error> {
    view.rendered().document.write(receipt)
}

// The JSON form of one payload under a receipt.
trait Document {
    fn write(&self, receipt: &Receipt<'_>) -> Result<String, Error>;
}

// The payload with its cut, and the envelope function that writes it.
struct JsonDocument<P> {
    content: WithTruncation<P>,
    write: Writer<P>,
}

impl<P> Document for JsonDocument<P> {
    fn write(&self, receipt: &Receipt<'_>) -> Result<String, Error> {
        (self.write)(receipt, &self.content, &receipt.ceiling())
    }
}

// The payload, with the service's cut as its last key when the service left items out.
#[derive(Serialize)]
pub(crate) struct WithTruncation<P> {
    #[serde(flatten)]
    payload: P,
    #[serde(skip_serializing_if = "Option::is_none")]
    truncation: Option<Truncation>,
}

impl<P> WithTruncation<P> {
    pub(crate) fn new(payload: P, truncation: Option<Truncation>) -> Self {
        let truncation = truncation.filter(|cut| cut.shown < cut.total);
        Self {
            payload,
            truncation,
        }
    }
}

impl<P: Listing> Listing for WithTruncation<P> {
    fn items(&self) -> usize {
        self.payload.items()
    }

    fn prefix(&self, kept: usize) -> Self {
        Self {
            payload: self.payload.prefix(kept),
            truncation: self.truncation,
        }
    }
}
