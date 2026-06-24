package runner

import (
	"context"
	"fmt"
	"slices"

	wrpc "wrpc.io/go"

	"driver/runner/test/lists/to_test"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	must0(to_test.EmptyListParam(ctx, c, []uint8{}))
	must0(to_test.EmptyStringParam(ctx, c, ""))
	assertEqual(0, len(must(to_test.EmptyListResult(ctx, c))))
	assertEqual(0, len(must(to_test.EmptyStringResult(ctx, c))))
	must0(to_test.ListParam(ctx, c, []uint8{1, 2, 3, 4}))
	must0(to_test.ListParam2(ctx, c, "foo"))
	must0(to_test.ListParam3(ctx, c, []string{"foo", "bar", "baz"}))
	must0(to_test.ListParam4(ctx, c, [][]string{{"foo", "bar"}, {"baz"}}))
	must0(to_test.ListParam5(ctx, c, []*wrpc.Tuple3[uint8, uint32, uint8]{
		{V0: 1, V1: 2, V2: 3},
		{V0: 4, V1: 5, V2: 6},
	}))

	large := make([]string, 0, 1000)
	for i := 0; i < 1000; i++ {
		large = append(large, "string")
	}
	must0(to_test.ListParamLarge(ctx, c, large))

	assert(slices.Equal(must(to_test.ListResult(ctx, c)), []uint8{1, 2, 3, 4, 5}))
	assertEqual(must(to_test.ListResult2(ctx, c)), "hello!")
	assert(slices.Equal(must(to_test.ListResult3(ctx, c)), []string{"hello,", "world!"}))
	assert(slices.Equal(must(to_test.ListRoundtrip(ctx, c, []uint8{})), []uint8{}))

	{
		headers := []*wrpc.Tuple2[string, []uint8]{
			{V0: "Content-Type", V1: []uint8("text/plain")},
			{V0: "Content-Length", V1: []uint8("9")},
		}
		result := must(to_test.WasiHttpHeadersRoundtrip(ctx, c, headers))
		assertEqual(len(result), 2)
		assertEqual(result[0].V0, "Content-Type")
		assert(slices.Equal(result[0].V1, []uint8("text/plain")))
		assertEqual(result[1].V0, "Content-Length")
		assert(slices.Equal(result[1].V1, []uint8("9")))
	}
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

func assertEqual[T comparable](a T, b T) {
	if a != b {
		panic(fmt.Sprintf("%v not equal to %v", a, b))
	}
}

func assert(v bool) {
	if !v {
		panic("assertion failed")
	}
}
