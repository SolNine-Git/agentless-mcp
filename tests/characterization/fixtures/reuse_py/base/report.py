def render_table(rows):
    widths = [max(map(len, column)) for column in zip(*rows)]
    lines = []
    for row in rows:
        cells = [cell.ljust(width) for cell, width in zip(row, widths)]
        lines.append(" | ".join(cells).rstrip())
    return "\n".join(lines)
