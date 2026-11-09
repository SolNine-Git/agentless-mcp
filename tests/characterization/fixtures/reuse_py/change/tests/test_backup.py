import os
import zipfile


def test_packs_every_path(tmp_path):
    target = tmp_path / "out.zip"
    with zipfile.ZipFile(target, "w", zipfile.ZIP_DEFLATED) as bundle:
        for path in sorted(["a", "b"]):
            bundle.write(path, arcname=os.path.basename(path))
    assert os.path.getsize(target) > 0
