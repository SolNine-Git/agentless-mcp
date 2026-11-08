def format_grid(rows, title):
    widths = [max(map(len, column)) for column in zip(*rows)]
    lines = [title.center(sum(widths))]
    for row in rows:
        cells = [cell.ljust(width) for cell, width in zip(row, widths)]
        lines.append(" ; ".join(cells).rstrip())
    return "\n".join(lines)
