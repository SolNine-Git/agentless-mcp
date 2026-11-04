from helpers import inv_footer, inv_header, inv_lines, inv_tax



def render_invoice(total):
    head = inv_header(total)
    body = inv_lines(head)
    taxed = inv_tax(body)
    return inv_footer(taxed)
