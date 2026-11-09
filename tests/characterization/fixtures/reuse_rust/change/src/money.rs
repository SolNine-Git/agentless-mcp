use std::fmt;

pub struct Price {
    cents: i64,
    currency: String,
}

impl fmt::Display for Price {
    fn fmt(&self, out: &mut fmt::Formatter) -> fmt::Result {
        out.write_str(&self.currency.to_uppercase())?;
        out.write_fmt(format_args!(" {}", self.cents))
    }
}

pub struct Cost {
    cents: i64,
    currency: String,
}

impl fmt::Display for Cost {
    fn fmt(&self, out: &mut fmt::Formatter) -> fmt::Result {
        out.write_str(&self.currency.to_uppercase())?;
        out.write_fmt(format_args!(" {}", self.cents))
    }
}
