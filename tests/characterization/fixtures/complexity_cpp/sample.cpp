namespace geo {
class Shape {
 public:
  int area(int w, int h) {
    // A comment line does not count.

    const char *label = R"(one
two
three)";
    auto clamp = [](int v) { return v > 0 and v < 100 ? v : 0; };
    for (int x : {w, h}) {
      if (x < 0 or x > 1000) {
        return -1;
      }
    }
    return clamp(w) * clamp(h);
  }
};
}  // namespace geo

int geo::Shape::scale(int f) {
  try {
    return f > 1 ? f : 1;
  } catch (...) {
    return 0;
  }
}
