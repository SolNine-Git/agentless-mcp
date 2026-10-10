class Store {
  private load = (id: string): number => (id ? 1 : 0);

  find(key: string): number {
    // A comment line does not count.

    const label = `one
two
three`;
    switch (key) {
      case "a":
        return 1;
      default:
        return key.length > 2 || key === "z" ? 2 : 3;
    }
  }
}

function* ids(n: number): Generator<number> {
  for (let i = 0; i < n; i++) {
    yield i;
  }
}
