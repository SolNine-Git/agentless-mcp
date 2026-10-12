//! Message templates as data, and the one function that fills them in.

use std::fmt::Display;
use std::marker::PhantomData;

/// The placeholder kind that takes text.
#[derive(Clone, Copy, Debug)]
pub struct Text;

/// The placeholder kind that takes a count.
#[derive(Clone, Copy, Debug)]
pub struct Count;

/// The value type that one placeholder kind takes.
pub trait Kind {
    /// The value that fills a placeholder of this kind.
    type Value<'a>: Display;
}

impl Kind for Text {
    type Value<'a> = &'a str;
}

impl Kind for Count {
    type Value<'a> = usize;
}

/// The values that a template takes, in value order: one kind, or a tuple of 2 to 6 kinds.
pub trait Signature {
    /// The values, one per placeholder name.
    type Values<'a>;

    /// Returns each value as text, in value order.
    fn texts(values: Self::Values<'_>) -> Vec<String>;
}

impl<K: Kind> Signature for K {
    type Values<'a> = K::Value<'a>;

    fn texts(values: Self::Values<'_>) -> Vec<String> {
        vec![values.to_string()]
    }
}

impl<A: Kind, B: Kind> Signature for (A, B) {
    type Values<'a> = (A::Value<'a>, B::Value<'a>);

    fn texts(values: Self::Values<'_>) -> Vec<String> {
        vec![values.0.to_string(), values.1.to_string()]
    }
}

impl<A: Kind, B: Kind, C: Kind> Signature for (A, B, C) {
    type Values<'a> = (A::Value<'a>, B::Value<'a>, C::Value<'a>);

    fn texts(values: Self::Values<'_>) -> Vec<String> {
        vec![
            values.0.to_string(),
            values.1.to_string(),
            values.2.to_string(),
        ]
    }
}

impl<A: Kind, B: Kind, C: Kind, D: Kind> Signature for (A, B, C, D) {
    type Values<'a> = (A::Value<'a>, B::Value<'a>, C::Value<'a>, D::Value<'a>);

    fn texts(values: Self::Values<'_>) -> Vec<String> {
        vec![
            values.0.to_string(),
            values.1.to_string(),
            values.2.to_string(),
            values.3.to_string(),
        ]
    }
}

impl<A: Kind, B: Kind, C: Kind, D: Kind, E: Kind> Signature for (A, B, C, D, E) {
    type Values<'a> = (
        A::Value<'a>,
        B::Value<'a>,
        C::Value<'a>,
        D::Value<'a>,
        E::Value<'a>,
    );

    fn texts(values: Self::Values<'_>) -> Vec<String> {
        vec![
            values.0.to_string(),
            values.1.to_string(),
            values.2.to_string(),
            values.3.to_string(),
            values.4.to_string(),
        ]
    }
}

impl<A: Kind, B: Kind, C: Kind, D: Kind, E: Kind, F: Kind> Signature for (A, B, C, D, E, F) {
    type Values<'a> = (
        A::Value<'a>,
        B::Value<'a>,
        C::Value<'a>,
        D::Value<'a>,
        E::Value<'a>,
        F::Value<'a>,
    );

    fn texts(values: Self::Values<'_>) -> Vec<String> {
        vec![
            values.0.to_string(),
            values.1.to_string(),
            values.2.to_string(),
            values.3.to_string(),
            values.4.to_string(),
            values.5.to_string(),
        ]
    }
}

pub(crate) enum Piece {
    Text(&'static str),
    Value(usize),
}

/// A message template whose placeholders take the values that `S` names.
pub struct Template<S> {
    pieces: &'static [Piece],
    signature: PhantomData<S>,
}

impl<S: Signature> Template<S> {
    pub(crate) const fn new(pieces: &'static [Piece]) -> Self {
        Self {
            pieces,
            signature: PhantomData,
        }
    }

    /// Returns the template text with each placeholder replaced by its value.
    #[must_use]
    pub fn render(&self, values: S::Values<'_>) -> String {
        fill(self.pieces, &S::texts(values))
    }

    #[cfg(test)]
    pub(crate) const fn pieces(&self) -> &'static [Piece] {
        self.pieces
    }
}

pub(crate) fn fill(pieces: &[Piece], values: &[String]) -> String {
    let mut text = String::new();
    for piece in pieces {
        match *piece {
            Piece::Text(literal) => text.push_str(literal),
            Piece::Value(position) => text.push_str(&values[position]),
        }
    }
    text
}
