function scaledSum(left: number, right: number): number {
  const total = left + right;
  console.log("sum", total);
  return total * 2;
}

function doubledTotal(a: number, b: number): number {
  const sum = a + b;
  console.log("total", sum);
  return sum * 3;
}

function nearMiss(left: number, right: number): number {
  const total = left - right;
  console.log("sum", total);
  return total * 2;
}
