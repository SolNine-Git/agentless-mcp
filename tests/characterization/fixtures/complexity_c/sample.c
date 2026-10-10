#include <stddef.h>

static int *find(int *xs, size_t n, int key) {
  /* A comment line does not count. */

  const char *label = "one"
                      "two";
  for (size_t i = 0; i < n; i++) {
    if (xs[i] == key && key != 0) {
      return &xs[i];
    }
  }
  return NULL;
}

int classify(int v) {
  int r = 0;
  switch (v) {
  case 1:
  case 2:
    r = v > 1 ? 2 : 1;
    break;
  default:
    r = -1;
  }
  while (r > 5 || r < -5) {
    r /= 2;
  }
  do {
    r++;
  } while (r < 0);
  return r;
}
