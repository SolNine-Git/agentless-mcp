pub struct Panel {
    layout: Layout,
    defaults: Sizes,
}

impl Panel {
    pub fn width(&self) -> u32 {
        self.layout.get("width").copied().unwrap_or(self.defaults.width)
    }

    pub fn height(&self) -> u32 {
        self.layout.get("height").copied().unwrap_or(self.defaults.height)
    }

    pub fn depth(&self) -> u32 {
        self.layout.get("depth").copied().unwrap_or(self.defaults.depth)
    }

    pub fn margin(&self) -> u32 {
        self.layout.get("margin").copied().unwrap_or(self.defaults.margin)
    }
}
