struct Parser {
    pos: usize,
}

impl Parser {
    fn next(&mut self, v: Option<i32>, flag: bool) -> i32 {
        // A comment line does not count.

        let _label = "one
two
three";
        if let Some(x) = v && (flag || self.pos > 3) {
            return x;
        }
        match v {
            Some(n) if n > 10 => n,
            Some(0) => 0,
            _ => -1,
        }
    }

    fn drain(&mut self) {
        let step = |p: usize| if p > 0 { p - 1 } else { 0 };
        while self.pos > 0 {
            self.pos = step(self.pos);
        }
        loop {
            if self.pos == 0 {
                break;
            }
        }
        for _ in 0..3 {}
    }
}

fn outer() -> i32 {
    fn inner(a: bool) -> i32 {
        if a { 1 } else { 2 }
    }
    inner(true) + inner(false)
}
