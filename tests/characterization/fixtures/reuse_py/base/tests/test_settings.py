from settings import load_settings


def test_reads_the_verbose_flag(tmp_path):
    path = tmp_path / "s.json"
    path.write_text('{"verbose": true}')
    assert load_settings(path)["verbose"] is True
