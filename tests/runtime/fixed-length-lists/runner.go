//@ wasmtime-flags = '-Wcomponent-model-fixed-length-lists'

package runner

import (
	"context"
	"fmt"

	wrpc "wrpc.io/go"

	"driver/runner/test/fixed_length_lists/to_test"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	must0(to_test.ListParam(ctx, c, [4]uint32{1, 2, 3, 4}))
	must0(to_test.ListParam2(ctx, c, [2][2]uint32{{1, 2}, {3, 4}}))
	must0(to_test.ListParam3(ctx, c, [20]int32{
		-1, 2, -3, 4, -5, 6, -7, 8, -9, 10, -11, 12, -13, 14, -15, 16, -17, 18, -19, 20,
	}))

	assertEqual(must(to_test.ListResult(ctx, c)), [8]uint8{'0', '1', 'A', 'B', 'a', 'b', 128, 255})

	{
		a, b := must2(to_test.ListMinmax16(ctx, c, [4]uint16{0, 1024, 32768, 65535}, [4]int16{1, 2048, -32767, -2}))
		assertEqual(a, [4]uint16{0, 1024, 32768, 65535})
		assertEqual(b, [4]int16{1, 2048, -32767, -2})
	}

	{
		a, b := must2(to_test.ListMinmaxFloat(ctx, c, [2]float32{2.0, -42.0}, [2]float64{0.25, -0.125}))
		assertEqual(a, [2]float32{2.0, -42.0})
		assertEqual(b, [2]float64{0.25, -0.125})
	}

	assertEqual(
		must(to_test.ListRoundtrip(ctx, c, [12]uint8{'a', 'b', 'c', 'd', 0, 1, 2, 3, 'A', 'B', 'Y', 'Z'})),
		[12]uint8{'a', 'b', 'c', 'd', 0, 1, 2, 3, 'A', 'B', 'Y', 'Z'},
	)

	{
		a, b := must2(to_test.NestedRoundtrip(ctx, c, [2][2]uint32{{1, 5}, {42, 1_000_000}}, [2][2]int32{{-1, 3}, {-2_000_000, 4711}}))
		assertEqual(a, [2][2]uint32{{1, 5}, {42, 1_000_000}})
		assertEqual(b, [2][2]int32{{-1, 3}, {-2_000_000, 4711}})
	}

	{
		large := [4][4]int32{
			{-1, 3, -2, 4},
			{-2_000_000, 4711, 99_999, -5},
			{-6, 7, 8, -9},
			{50, -5, 500, -5000},
		}
		a, b := must2(to_test.LargeRoundtrip(ctx, c, [2][2]uint32{{1, 5}, {42, 1_000_000}}, large))
		assertEqual(a, [2][2]uint32{{1, 5}, {42, 1_000_000}})
		assertEqual(b, large)
	}

	result := must(to_test.NightmareOnCpp(ctx, c, [2]*to_test.Nested{{L: [2]int32{1, -1}}, {L: [2]int32{2, -2}}}))
	assertEqual(result[0].L, [2]int32{1, -1})
	assertEqual(result[1].L, [2]int32{2, -2})
	return nil
}

func must[T any](v T, err error) T {
	if err != nil {
		panic(err)
	}
	return v
}

func must0(err error) {
	if err != nil {
		panic(err)
	}
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
