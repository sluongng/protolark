"""Behavioral tests evaluating generated constructors in Bazel's Starlark."""

load("@bazel_skylib//lib:unittest.bzl", "analysistest", "asserts", "unittest")
load("//protolark:runtime.bzl", "create_message", "wkt")
load(":project_generated.golden.bzl", "common_proto", "project_proto")
load(":runtime_load.scl", scl_value = "value", scl_wkt_value = "wkt_value")

def _constructors_impl(ctx):
    env = unittest.begin(ctx)
    asserts.equals(env, "shared-scl", scl_value.name)
    targets = ["//app:all"]
    labels = {"team": "build"}
    value = project_proto.Project.create(
        name = "test",
        targets = targets,
        labels = labels,
        owner = common_proto.Owner.create(name = "owner"),
        mode = project_proto.Project.Mode.FAST,
        settings = project_proto.Project.Settings.create(timeout_seconds = 30),
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
    absent = project_proto.Project.create(name = None)
    asserts.equals(env, [], dir(absent))
    asserts.false(env, hasattr(absent, "enabled"))
    asserts.equals(env, 0, scl_wkt_value.struct_value.fields["null"].null_value)
    assert_values = scl_wkt_value.struct_value.fields["items"].list_value.values
    asserts.equals(env, True, assert_values[0].bool_value)
    asserts.equals(env, 7, assert_values[1].number_value)
    asserts.equals(env, "text", assert_values[2].string_value)
    asserts.equals(env, -62135596800, wkt.timestamp(seconds = -62135596800).seconds)
    asserts.equals(env, 999999999, wkt.timestamp(seconds = 253402300799, nanos = 999999999).nanos)
    asserts.equals(env, -999999999, wkt.duration(seconds = -315576000000, nanos = -999999999).nanos)
    asserts.equals(env, 315576000000, wkt.duration(seconds = 315576000000, nanos = 999999999).seconds)
    asserts.equals(env, -1, wkt.duration(nanos = -1).nanos)
    asserts.equals(env, 0, wkt.timestamp().seconds)
    asserts.equals(env, 0, wkt.value(None).null_value)
    asserts.equals(env, 9007199254740991, wkt.value(9007199254740991).number_value)
    asserts.equals(env, -9007199254740991, wkt.value(-9007199254740991).number_value)
    asserts.equals(env, {}, wkt.struct({}).fields)
    asserts.equals(env, [], wkt.list_value(()).values)
    values = [False, {"empty": [], "text": "hello"}, ()]
    converted = wkt.list_value(values)
    values.append("later")
    asserts.equals(env, 3, len(converted.values))
    asserts.equals(env, False, converted.values[0].bool_value)
    asserts.equals(env, [], converted.values[1].struct_value.fields["empty"].list_value.values)
    asserts.equals(env, "hello", converted.values[1].struct_value.fields["text"].string_value)
    asserts.equals(env, [], converted.values[2].list_value.values)
    nested = "leaf"
    for _ in range(20):
        nested = [nested]
    nested_value = wkt.value(nested)
    for _ in range(20):
        nested_value = nested_value.list_value.values[0]
    asserts.equals(env, "leaf", nested_value.string_value)
    return unittest.end(env)

_constructors_test = unittest.make(_constructors_impl)

def _invalid_impl(ctx):
    if ctx.attr.case == "unknown":
        project_proto.Project.create(unknown = True)
    elif ctx.attr.case == "type":
        project_proto.Project.create(name = 123)
    elif ctx.attr.case == "oneof":
        project_proto.Project.create(local_path = "a", remote_url = "b")
    elif ctx.attr.case == "range":
        project_proto.Project.Settings.create(timeout_seconds = 1 << 31)
    elif ctx.attr.case == "enum":
        project_proto.Project.create(mode = 99)
    elif ctx.attr.case == "timestamp_seconds":
        wkt.timestamp(seconds = 253402300800)
    elif ctx.attr.case == "timestamp_lower":
        wkt.timestamp(seconds = -62135596801)
    elif ctx.attr.case == "timestamp_nanos":
        wkt.timestamp(nanos = -1)
    elif ctx.attr.case == "timestamp_nanos_upper":
        wkt.timestamp(nanos = 1000000000)
    elif ctx.attr.case == "duration_seconds":
        wkt.duration(seconds = 315576000001)
    elif ctx.attr.case == "duration_lower":
        wkt.duration(seconds = -315576000001)
    elif ctx.attr.case == "duration_nanos":
        wkt.duration(nanos = -1000000000)
    elif ctx.attr.case == "duration_nanos_upper":
        wkt.duration(nanos = 1000000000)
    elif ctx.attr.case == "duration_sign":
        wkt.duration(seconds = 1, nanos = -1)
    elif ctx.attr.case == "duration_sign_negative":
        wkt.duration(seconds = -1, nanos = 1)
    elif ctx.attr.case == "empty_value":
        create_message("google.protobuf.Value", {}, {})
    elif ctx.attr.case == "value_key":
        wkt.value({1: "invalid"})
    elif ctx.attr.case == "value_type":
        wkt.value(struct(field = "invalid"))
    elif ctx.attr.case == "value_integer":
        wkt.value(9007199254740992)
    elif ctx.attr.case == "value_integer_negative":
        wkt.value(-9007199254740992)
    elif ctx.attr.case == "struct_type":
        wkt.struct([])
    elif ctx.attr.case == "list_type":
        wkt.list_value({})
    elif ctx.attr.case == "value_depth":
        nested = None
        for _ in range(21):
            nested = [nested]
        wkt.value(nested)
    elif ctx.attr.case == "value_expansion":
        shared = None
        for _ in range(18):
            shared = [shared, shared]
        wkt.value(shared)
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
        "timestamp_seconds": "google.protobuf.Timestamp.seconds: must be between",
        "timestamp_lower": "google.protobuf.Timestamp.seconds: must be between",
        "timestamp_nanos": "google.protobuf.Timestamp.nanos: must be between",
        "timestamp_nanos_upper": "google.protobuf.Timestamp.nanos: must be between",
        "duration_seconds": "google.protobuf.Duration.seconds: must be between",
        "duration_lower": "google.protobuf.Duration.seconds: must be between",
        "duration_nanos": "google.protobuf.Duration.nanos: must be between",
        "duration_nanos_upper": "google.protobuf.Duration.nanos: must be between",
        "duration_sign": "seconds and nanos must have consistent signs",
        "duration_sign_negative": "seconds and nanos must have consistent signs",
        "empty_value": "exactly one kind must be selected",
        "value_key": "dict keys must be strings",
        "value_type": "unsupported type struct",
        "value_integer": "integers must be within the exact double range",
        "value_integer_negative": "integers must be within the exact double range",
        "struct_type": "wkt.struct: expected dict",
        "list_type": "wkt.list_value: expected list or tuple",
        "value_depth": "maximum nesting depth is 20 containers",
        "value_expansion": "maximum expanded value count is 100000",
    }
    for case, error in cases.items():
        _invalid(name = name + "_bad_" + case, case = case, tags = ["manual"])
        _failure_test(name = name + "_" + case, target_under_test = ":" + name + "_bad_" + case, error = error)
    native.test_suite(name = name, tests = [name + "_valid"] + [name + "_" + case for case in cases])
