fn flag(value: i64) -> i64 {
    value * 6
}

fn describe(value: i64) -> i64 {
    value * 7
}

fn register(value: i64) -> i64 {
    value * 8
}

pub fn arg_alpha() -> i64 {
    register(describe(flag(1)))
}

pub fn arg_beta() -> i64 {
    register(describe(flag(2)))
}

pub fn arg_gamma() -> i64 {
    register(describe(flag(3)))
}

pub fn arg_delta() -> i64 {
    register(describe(flag(4)))
}

pub fn arg_epsilon() -> i64 {
    register(describe(flag(5)))
}

pub fn arg_zeta() -> i64 {
    register(describe(flag(6)))
}

pub fn arg_eta() -> i64 {
    register(describe(flag(7)))
}
