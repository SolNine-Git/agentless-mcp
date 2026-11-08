import heapq
import operator


def latest_record(records):
    kept = [record for record in records if record.stamp is not None]
    top = heapq.nlargest(1, kept, key=operator.attrgetter("stamp"))
    return top[0] if top else None
