const Row = ({ ok }: { ok: boolean }) => {
  // A comment line does not count.

  const label = `one
two
three`;
  return ok && <span>{ok ? "yes" : "no"}</span>;
};

export function List(props: { items: string[] }) {
  return (
    <ul>
      {props.items.map((item) => (item ? <li>{item}</li> : null))}
    </ul>
  );
}
