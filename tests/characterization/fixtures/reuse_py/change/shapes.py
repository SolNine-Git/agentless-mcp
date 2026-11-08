class Panel:
    def __init__(self, layout, defaults):
        self.layout = layout
        self.defaults = defaults

    def width(self):
        return self.layout.get("width", self.defaults.get("width"))

    def height(self):
        return self.layout.get("height", self.defaults.get("height"))

    def depth(self):
        return self.layout.get("depth", self.defaults.get("depth"))

    def margin(self):
        return self.layout.get("margin", self.defaults.get("margin"))

    def padding(self):
        return self.layout.get("padding", self.defaults.get("padding"))
