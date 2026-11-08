import os
import zipfile


def pack_archive(paths, target):
    with zipfile.ZipFile(target, "w", zipfile.ZIP_DEFLATED) as bundle:
        for path in sorted(paths):
            bundle.write(path, arcname=os.path.basename(path))
    return os.path.getsize(target)
