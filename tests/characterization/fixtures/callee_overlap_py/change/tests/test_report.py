from report import body, footer, header, totals



def test_renders_a_report():
    assert totals(footer(body(header(1)))) == 7
