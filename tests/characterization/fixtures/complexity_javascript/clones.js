function scaledSum(left, right) {
  const total = left + right;
  console.log("sum", total);
  return total * 2;
}

function doubledTotal(a, b) {
  const sum = a + b;
  console.log("total", sum);
  return sum * 3;
}

function nearMiss(left, right) {
  const total = left - right;
  console.log("sum", total);
  return total * 2;
}
