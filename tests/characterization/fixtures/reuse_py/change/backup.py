import os
import zipfile

from archive import pack_archive


def backup_all(paths, target):
    kept = [path for path in sorted(paths) if os.path.basename(path) != ".lock"]
    size = pack_archive(kept, target)
    with zipfile.ZipFile(target) as bundle:
        bundle.write(target + ".sum", arcname=os.path.basename(target))
    return size, os.path.getsize(target)
