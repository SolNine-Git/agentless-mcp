//! The body map: the function map's file rows, then the bodies of its best symbols, cut to the
//! map's own budget, in one answer.

use io::Granularity;

use super::{MapRequest, MapResult, map, map_rows_text};
use crate::expand::{EXPAND_MAX_SEATS, ExpandResult, Seating, expand_symbols, first_copies};
use crate::{Error, MapFile, Noted, Reading, Services};

// The tokens one body seat stands for: the map's budget over this is the seat count.
const BODY_TOKENS_PER_SEAT: usize = 300;

/// The map's file rows without their symbol rows, and the bodies of the map's best symbols.
#[derive(Clone, Debug, PartialEq)]
pub struct BodyMapResult {
    /// The map, each file stripped of its symbol rows, with the token count of what remains.
    pub map: MapResult,
    /// The bodies, focus symbols first, then by the packing's score.
    pub bodies: ExpandResult,
}

/// Builds the function map that `request` asks for, then expands its top symbols. The scan's
/// warnings come with either outcome.
#[must_use]
pub fn body_map(
    services: &Services,
    reading: &Reading<'_>,
    request: &MapRequest,
) -> Noted<Result<BodyMapResult, Error>> {
    let function = MapRequest {
        granularity: Some(Granularity::Function),
        ..request.clone()
    };
    let built = map(services, reading, &function);
    let mut diagnostics = built.diagnostics;
    let base = match built.value {
        Ok(base) => base,
        Err(error) => {
            return Noted {
                value: Err(error),
                diagnostics,
            };
        }
    };
    let order: Vec<String> = base
        .focus_order
        .iter()
        .chain(&base.expand_order)
        .cloned()
        .collect();
    let ids: Vec<String> = first_copies(&order)
        .into_iter()
        .map(str::to_owned)
        .collect();
    let seats = (base.budget / BODY_TOKENS_PER_SEAT).clamp(1, EXPAND_MAX_SEATS);
    let seating = Seating {
        limit: seats,
        budget: base.budget,
        seats,
    };
    let seated = &ids[..seats.min(ids.len())];
    let bodies = expand_symbols(services, reading, seated, seating);
    diagnostics.extend(bodies.diagnostics);
    let files: Vec<MapFile> = (base.files.into_iter())
        .map(|file| MapFile {
            entries: Vec::new(),
            ..file
        })
        .collect();
    let rendered = services.counter.count(&map_rows_text(&files));
    let map = MapResult {
        files,
        rendered,
        ..base
    };
    Noted {
        value: Ok(BodyMapResult {
            map,
            bodies: bodies.value,
        }),
        diagnostics,
    }
}
