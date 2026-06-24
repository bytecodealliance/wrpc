//@ wasmtime-flags = '-Wcomponent-model-map'

package runner

import (
	"context"
	"fmt"

	wrpc "wrpc.io/go"

	"driver/runner/test/maps/to_test"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	testNamedRoundtrip(ctx, c)
	testBytesRoundtrip(ctx, c)
	testEmptyRoundtrip(ctx, c)
	testOptionRoundtrip(ctx, c)
	testRecordRoundtrip(ctx, c)
	testInlineRoundtrip(ctx, c)
	testLargeRoundtrip(ctx, c)
	testMultiParamRoundtrip(ctx, c)
	testNestedRoundtrip(ctx, c)
	testVariantRoundtrip(ctx, c)
	testResultRoundtrip(ctx, c)
	testTupleRoundtrip(ctx, c)
	testSingleEntryRoundtrip(ctx, c)
	return nil
}

func testNamedRoundtrip(ctx context.Context, c wrpc.Invoker) {
	input := to_test.NamesById{
		1: "uno",
		2: "two",
	}
	result := must(to_test.NamedRoundtrip(ctx, c, input))
	assertEqual(result["uno"], uint32(1))
	assertEqual(result["two"], uint32(2))
}

func testBytesRoundtrip(ctx context.Context, c wrpc.Invoker) {
	input := to_test.BytesByName{
		"hello": []uint8("world"),
		"bin":   {0, 1, 2},
	}
	result := must(to_test.BytesRoundtrip(ctx, c, input))
	assertSliceEqual(result["hello"], []uint8("world"))
	assertSliceEqual(result["bin"], []uint8{0, 1, 2})
}

func testEmptyRoundtrip(ctx context.Context, c wrpc.Invoker) {
	result := must(to_test.EmptyRoundtrip(ctx, c, to_test.NamesById{}))
	assertEqual(len(result), 0)
}

func testOptionRoundtrip(ctx context.Context, c wrpc.Invoker) {
	input := map[string]*uint32{
		"some": ptr[uint32](42),
		"none": nil,
	}
	result := must(to_test.OptionRoundtrip(ctx, c, input))
	assertEqual(len(result), 2)
	assertEqual(*result["some"], uint32(42))
	assertNil(result["none"])
}

func testRecordRoundtrip(ctx context.Context, c wrpc.Invoker) {
	entry := &to_test.LabeledEntry{
		Label: "test-label",
		Values: to_test.NamesById{
			10: "ten",
			20: "twenty",
		},
	}
	result := must(to_test.RecordRoundtrip(ctx, c, entry))
	assertEqual(result.Label, "test-label")
	assertEqual(len(result.Values), 2)
	assertEqual(result.Values[10], "ten")
	assertEqual(result.Values[20], "twenty")
}

func testInlineRoundtrip(ctx context.Context, c wrpc.Invoker) {
	input := map[uint32]string{
		1: "one",
		2: "two",
	}
	result := must(to_test.InlineRoundtrip(ctx, c, input))
	assertEqual(len(result), 2)
	assertEqual(result["one"], uint32(1))
	assertEqual(result["two"], uint32(2))
}

func testLargeRoundtrip(ctx context.Context, c wrpc.Invoker) {
	input := make(to_test.NamesById)
	for i := uint32(0); i < 100; i++ {
		input[i] = fmt.Sprintf("value-%d", i)
	}
	result := must(to_test.LargeRoundtrip(ctx, c, input))
	assertEqual(len(result), 100)
	for i := uint32(0); i < 100; i++ {
		assertEqual(result[i], fmt.Sprintf("value-%d", i))
	}
}

func testMultiParamRoundtrip(ctx context.Context, c wrpc.Invoker) {
	names := to_test.NamesById{
		1: "one",
		2: "two",
	}
	bytes := to_test.BytesByName{
		"key": {42},
	}
	ids, bytesOut := must2(to_test.MultiParamRoundtrip(ctx, c, names, bytes))
	assertEqual(len(ids), 2)
	assertEqual(ids["one"], uint32(1))
	assertEqual(ids["two"], uint32(2))
	assertEqual(len(bytesOut), 1)
	assertSliceEqual(bytesOut["key"], []uint8{42})
}

func testNestedRoundtrip(ctx context.Context, c wrpc.Invoker) {
	input := map[string]map[uint32]string{
		"group-a": {
			1: "one",
			2: "two",
		},
		"group-b": {
			10: "ten",
		},
	}
	result := must(to_test.NestedRoundtrip(ctx, c, input))
	assertEqual(len(result), 2)
	assertEqual(result["group-a"][1], "one")
	assertEqual(result["group-a"][2], "two")
	assertEqual(result["group-b"][10], "ten")
}

func testVariantRoundtrip(ctx context.Context, c wrpc.Invoker) {
	m := to_test.NamesById{1: "one"}
	asMap := must(to_test.VariantRoundtrip(ctx, c, to_test.NewMapOrStringAsMap(m)))
	assertEqual(asMap.GetAsMap()[1], "one")

	asStr := must(to_test.VariantRoundtrip(ctx, c, to_test.NewMapOrStringAsString("hello")))
	assertEqual(asStr.GetAsString(), "hello")
}

func testResultRoundtrip(ctx context.Context, c wrpc.Invoker) {
	m := to_test.NamesById{5: "five"}
	okResult := must(to_test.ResultRoundtrip(ctx, c, wrpc.Ok[string](m)))
	assertEqual(okResult.Ok[5], "five")

	errResult := must(to_test.ResultRoundtrip(ctx, c, wrpc.Err[to_test.NamesById]("bad input")))
	assertEqual(*errResult.Err, "bad input")
}

func testTupleRoundtrip(ctx context.Context, c wrpc.Invoker) {
	m := to_test.NamesById{7: "seven"}
	resultMap, resultNum := must2(to_test.TupleRoundtrip(ctx, c, &wrpc.Tuple2[to_test.NamesById, uint64]{V0: m, V1: 42}))
	assertEqual(len(resultMap), 1)
	assertEqual(resultMap[7], "seven")
	assertEqual(resultNum, uint64(42))
}

func testSingleEntryRoundtrip(ctx context.Context, c wrpc.Invoker) {
	input := to_test.NamesById{99: "ninety-nine"}
	result := must(to_test.SingleEntryRoundtrip(ctx, c, input))
	assertEqual(len(result), 1)
	assertEqual(result[99], "ninety-nine")
}

func ptr[T any](v T) *T {
	return &v
}

func must[T any](v T, err error) T {
	if err != nil {
		panic(err)
	}
	return v
}

func must2[A, B any](a A, b B, err error) (A, B) {
	if err != nil {
		panic(err)
	}
	return a, b
}

func assertEqual[T comparable](a T, b T) {
	if a != b {
		panic(fmt.Sprintf("%v not equal to %v", a, b))
	}
}

func assertSliceEqual[T comparable](a []T, b []T) {
	if len(a) != len(b) {
		panic(fmt.Sprintf("slices have different lengths: %d vs %d", len(a), len(b)))
	}
	for i := range a {
		if a[i] != b[i] {
			panic(fmt.Sprintf("slices differ at index %d: %v vs %v", i, a[i], b[i]))
		}
	}
}

func assertNil[T any](v *T) {
	if v != nil {
		panic(fmt.Sprintf("expected nil, got %v", *v))
	}
}
