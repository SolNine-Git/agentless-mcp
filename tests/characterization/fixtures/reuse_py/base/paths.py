import os
from pathlib import PurePosixPath


def common_root(paths):
    parts = [PurePosixPath(path).parts for path in paths]
    shared = os.path.commonprefix(parts)
    return PurePosixPath(*shared) if shared else None
