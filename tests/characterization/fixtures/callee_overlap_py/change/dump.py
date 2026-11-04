import helpers



def dump_rows(rows):
    out = helpers.exp_open(0)
    for row in rows:
        out += helpers.exp_row(helpers.exp_escape(row))
    return helpers.exp_close(out)
