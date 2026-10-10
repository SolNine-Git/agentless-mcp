class Sample {
  int route(Object o, int x) {
    // A comment line does not count.

    String label = """
        one
        two""";
    switch (x) {
      case 1:
      case 2, 3:
        break;
      default:
        return 0;
    }
    int y = switch (o) {
      case Integer i when i > 0 -> 1;
      case String s -> 2;
      default -> 3;
    };
    assert x > 0 && y > 0;
    return y;
  }

  void loop(int[] xs) {
    int x = xs.length;
    Runnable r = () -> {
      for (int v : xs) {
        if (v > 0 || v < -5) {
          continue;
        }
      }
    };
    try {
      r.run();
    } catch (RuntimeException e) {
      do {
        x = x > 0 ? x - 1 : 0;
      } while (x != 0);
    }
  }

  record Point(int x) {
    Point {
      if (x < 0) throw new IllegalArgumentException();
    }
  }
}
