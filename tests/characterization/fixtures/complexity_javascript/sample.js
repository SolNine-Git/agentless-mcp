// Declarations, methods, and bound callables.
function route(kind, flag) {
  // A comment line does not count.

  const label = `one
two
three`;
  if (kind === "a" && flag) {
    return 1;
  } else if (kind === "b" || (flag ?? false)) {
    return 2;
  }
  switch (kind) {
    case "c":
    case "d":
      return 3;
    default:
      return;
  }
}

const pick = (xs) => xs.filter((x) => (x > 0 ? x : null));

function outer(a) {
  function inner(b) {
    return b ? 1 : 2;
  }
  return a && inner(a);
}

class Box {
  size = () => {
    for (const v of this.items) {
      while (v) {
        break;
      }
    }
  };

  open() {
    try {
      run();
    } catch (err) {
      do {
        retry();
      } while (err && false);
    }
  }
}

const table = {
  load: function () {
    return this.x || 0;
  },
};

[1, 2].forEach(function () {
  if (ready) {
    go();
  }
});
