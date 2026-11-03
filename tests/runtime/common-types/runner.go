package runner

import (
	"context"
	"fmt"

	wrpc "wrpc.io/go"

	"driver/runner/test/common/test_types"
	"driver/runner/test/common/to_test"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	res := must(to_test.Wrap(ctx, c, &test_types.F1{A: true}))
	assertEqual(*res.B, test_types.F1{A: true})
	assertEqual(res.A, 1)

	res2 := must(to_test.Wrap(ctx, c, &test_types.F1{B: true}))
	assertEqual(*res2.B, test_types.F1{B: true})
	assertEqual(res2.A, 2)

	res3 := must(to_test.VarF(ctx, c))
	v, ok := res3.GetB()
	assert(ok)
	assertEqual(v, 42)
	return nil
}

func must[T any](v T, err error) T {
	if err != nil {
		panic(err)
	}
	return v
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
