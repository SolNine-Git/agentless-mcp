import csv

COLUMNS = ["name", "count"]


def dump_tsv(rows, path):
    with open(path, "w", newline="") as handle:
        writer = csv.writer(handle, delimiter="\t")
        writer.writerow(COLUMNS)
        writer.writerows(rows)
        handle.flush()
    return len(rows)
