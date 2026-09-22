"""Bazel entry point for the shared protobuf constructor runtime.

Generated .bzl files load this module through @protolark. The implementation is
shared with runtime.scl to keep validation identical in both dialects. Bazel
allows .bzl files to load external repositories and .scl files, but pure .scl
files (including PROJECT.scl) cannot load .bzl files or @repo labels. Those
consumers must use a same-repository copy of runtime.scl via a //... label.
The standalone Protolark evaluator supplies the standard runtime from its binary.
"""

load(":runtime.scl", _create_message = "create_message")

create_message = _create_message
