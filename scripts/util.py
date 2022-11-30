import sys


def eprint(*args, **kwargs):
    print(*args, file=sys.stderr, **kwargs)


def error(msg):
    eprint("error:", msg)
    sys.exit(1)
