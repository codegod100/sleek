#!/usr/bin/python3
"""Run zig cc after recursively expanding Buck response files."""

import os
import shlex
import sys


def expand(arguments):
    result = []
    for argument in arguments:
        if argument.startswith("@"):
            with open(argument[1:], encoding="utf-8") as response:
                result.extend(expand(shlex.split(response.read())))
        else:
            result.append(argument)
    return result


os.execv(sys.argv[1], [sys.argv[1], sys.argv[2], *expand(sys.argv[3:])])
