from pathlib import PurePosixPath


def shared_parent(files):
    split = [PurePosixPath(name).parts for name in files]
    first = split[0] if split else ()
    size = 0
    while size < len(first) and all(len(p) > size and p[size] == first[size] for p in split):
        size += 1
    return PurePosixPath(*first[:size]) if size else None
