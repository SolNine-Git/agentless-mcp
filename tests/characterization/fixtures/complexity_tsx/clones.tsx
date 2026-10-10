function Row(label: string) {
  const text = label.trim();
  return <li className="row">{text}</li>;
}

function Cell(title: string) {
  const value = title.trim();
  return <li className="cell">{value}</li>;
}

function NearMiss(label: string) {
  const text = label.trim().toUpperCase();
  return <li className="row">{text}</li>;
}
