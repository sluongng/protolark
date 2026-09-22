"""Behavioral tests evaluating generated constructors in Bazel's Starlark."""

load("@bazel_skylib//lib:unittest.bzl", "analysistest", "asserts", "unittest")
load(":project_generated.golden.bzl", "common_pb2", "project_pb2")
load(":runtime_load.scl", scl_value = "value")

def _constructors_impl(ctx):
    env = unittest.begin(ctx)
    asserts.equals(env, "shared-scl", scl_value.name)
    targets = ["//app:all"]
    labels = {"team": "build"}
    value = project_pb2.Project.create(
        name = "test",
        targets = targets,
        labels = labels,
        owner = common_pb2.Owner.create(name = "owner"),
        mode = project_pb2.Project.Mode.FAST,
        settings = project_pb2.Project.Settings.create(timeout_seconds = 30),
        enabled = False,
    )
    asserts.equals(env, "test", value.name)
    asserts.equals(env, "owner", value.owner.name)
    asserts.equals(env, 1, value.mode)
    asserts.equals(env, 30, value.settings.timeout_seconds)
    asserts.equals(env, False, value.enabled)
    targets.append("//other:all")
    labels["team"] = "changed"
    asserts.equals(env, ["//app:all"], value.targets)
    asserts.equals(env, {"team": "build"}, value.labels)
    absent = project_pb2.Project.create(name = None)
    asserts.equals(env, [], dir(absent))
    asserts.false(env, hasattr(absent, "enabled"))
    return unittest.end(env)

_constructors_test = unittest.make(_constructors_impl)

def _invalid_impl(ctx):
    if ctx.attr.case == "unknown":
        project_pb2.Project.create(unknown = True)
    elif ctx.attr.case == "type":
        project_pb2.Project.create(name = 123)
    elif ctx.attr.case == "oneof":
        project_pb2.Project.create(local_path = "a", remote_url = "b")
    elif ctx.attr.case == "range":
        project_pb2.Project.Settings.create(timeout_seconds = 1 << 31)
    elif ctx.attr.case == "enum":
        project_pb2.Project.create(mode = 99)
    return [DefaultInfo()]

_invalid = rule(implementation = _invalid_impl, attrs = {"case": attr.string()})

def _failure_impl(ctx):
    env = analysistest.begin(ctx)
    asserts.expect_failure(env, ctx.attr.error)
    return analysistest.end(env)

_failure_test = analysistest.make(_failure_impl, expect_failure = True, attrs = {"error": attr.string()})

def constructor_test_suite(name):
    """Instantiates valid constructor and invalid-input analysis tests.

    Args:
        name: Test suite name and prefix for its test targets.
    """
    _constructors_test(name = name + "_valid")
    cases = {
        "unknown": "unknown field unknown",
        "type": "expected string",
        "oneof": "multiple fields in oneof",
        "range": "integer out of range",
        "enum": "expected a declared enum number",
    }
    for case, error in cases.items():
        _invalid(name = name + "_bad_" + case, case = case, tags = ["manual"])
        _failure_test(name = name + "_" + case, target_under_test = ":" + name + "_bad_" + case, error = error)
    native.test_suite(name = name, tests = [name + "_valid"] + [name + "_" + case for case in cases])
