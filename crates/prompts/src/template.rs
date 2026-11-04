//! Message templates as data, and the one function that fills them in.

use std::fmt::Display;
use std::marker::PhantomData;

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

    /// Returns the template text with each placeholder replaced by its value, and each choice by
    /// the form its count takes.
    #[must_use]
    pub fn render(&self, values: S::Values<'_>) -> String {
        fill(self.pieces, &S::filled(values))
    }

    #[cfg(test)]
    pub(crate) const fn pieces(&self) -> &'static [Piece] {
        self.pieces
    }
}

pub(crate) enum Piece {
    Text(&'static str),
    Value(usize),
    // The position of a count, its form for a count of one, and its form for any other count.
    Choice(usize, &'static str, &'static str),
}

pub(crate) fn fill(pieces: &[Piece], values: &[Filled]) -> String {
    let mut text = String::new();
    for piece in pieces {
        match *piece {
            Piece::Text(literal) => text.push_str(literal),
            Piece::Value(position) => text.push_str(&values[position].text),
            Piece::Choice(position, one, many) => {
                text.push_str(if values[position].one { one } else { many });
            }
        }
    }
    text
}

/// The text of one value, and whether the value is a count of one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filled {
    text: String,
    one: bool,
}

impl Filled {
    pub(crate) fn new(text: String, one: bool) -> Self {
        Self { text, one }
    }
}

fn filled<K: Kind>(value: &K::Value<'_>) -> Filled {
    Filled::new(value.to_string(), K::is_one(value))
}

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

    /// Returns true when the value is a count of one, which takes the singular form.
    fn is_one(value: &Self::Value<'_>) -> bool;
}

impl Kind for Text {
    type Value<'a> = &'a str;

    fn is_one(_: &&str) -> bool {
        false
    }
}

impl Kind for Count {
    type Value<'a> = usize;

    fn is_one(value: &usize) -> bool {
        *value == 1
    }
}

/// The values that a template takes, in value order: one kind, or a tuple of 2 to 6 kinds.
pub trait Signature {
    /// The values, one per placeholder name.
    type Values<'a>;

    /// Returns each value as text with whether it is a count of one, in value order.
    fn filled(values: Self::Values<'_>) -> Vec<Filled>;
}

impl<K: Kind> Signature for K {
    type Values<'a> = K::Value<'a>;

    fn filled(values: Self::Values<'_>) -> Vec<Filled> {
        vec![filled::<K>(&values)]
    }
}

impl<A: Kind, B: Kind> Signature for (A, B) {
    type Values<'a> = (A::Value<'a>, B::Value<'a>);

    fn filled(values: Self::Values<'_>) -> Vec<Filled> {
        vec![filled::<A>(&values.0), filled::<B>(&values.1)]
    }
}

impl<A: Kind, B: Kind, C: Kind> Signature for (A, B, C) {
    type Values<'a> = (A::Value<'a>, B::Value<'a>, C::Value<'a>);

    fn filled(values: Self::Values<'_>) -> Vec<Filled> {
        vec![
            filled::<A>(&values.0),
            filled::<B>(&values.1),
            filled::<C>(&values.2),
        ]
    }
}

impl<A: Kind, B: Kind, C: Kind, D: Kind> Signature for (A, B, C, D) {
    type Values<'a> = (A::Value<'a>, B::Value<'a>, C::Value<'a>, D::Value<'a>);

    fn filled(values: Self::Values<'_>) -> Vec<Filled> {
        vec![
            filled::<A>(&values.0),
            filled::<B>(&values.1),
            filled::<C>(&values.2),
            filled::<D>(&values.3),
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

    fn filled(values: Self::Values<'_>) -> Vec<Filled> {
        vec![
            filled::<A>(&values.0),
            filled::<B>(&values.1),
            filled::<C>(&values.2),
            filled::<D>(&values.3),
            filled::<E>(&values.4),
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

    fn filled(values: Self::Values<'_>) -> Vec<Filled> {
        vec![
            filled::<A>(&values.0),
            filled::<B>(&values.1),
            filled::<C>(&values.2),
            filled::<D>(&values.3),
            filled::<E>(&values.4),
            filled::<F>(&values.5),
        ]
    }
}
