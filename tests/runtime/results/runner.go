package runner

import (
	"context"
	"fmt"

	wrpc "wrpc.io/go"

	"driver/runner/test/results/test"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	assertEqual(*must(test.StringError(ctx, c, 0.0)).Err, "zero")
	assertEqual(*must(test.StringError(ctx, c, 1.0)).Ok, 1.0)

	assertEqual(*must(test.EnumError(ctx, c, 0.0)).Err, test.E_A)
	assertEqual(*must(test.EnumError(ctx, c, 1.0)).Ok, 1.0)

	assertEqual(*must(test.RecordError(ctx, c, 0.0)).Err, test.E2{Line: 420, Column: 0})
	assertEqual(*must(test.RecordError(ctx, c, 1.0)).Err, test.E2{Line: 77, Column: 2})
	assert(must(test.RecordError(ctx, c, 2.0)).Ok != nil)

	e2, ok := must(test.VariantError(ctx, c, 0.0)).Err.GetE2()
	assert(ok)
	assertEqual(*e2, test.E2{Line: 420, Column: 0})
	e1, ok := must(test.VariantError(ctx, c, 1.0)).Err.GetE1()
	assert(ok)
	assertEqual(e1, test.E_B)
	e1, ok = must(test.VariantError(ctx, c, 2.0)).Err.GetE1()
	assert(ok)
	assertEqual(e1, test.E_C)

	assert(must(test.EmptyError(ctx, c, 0)).Err != nil)
	assertEqual(*must(test.EmptyError(ctx, c, 1)).Ok, 42)
	assertEqual(*must(test.EmptyError(ctx, c, 2)).Ok, 2)

	assert(must(test.DoubleError(ctx, c, 0)).Ok.Ok != nil)
	assertEqual(*must(test.DoubleError(ctx, c, 1)).Ok.Err, "one")
	assertEqual(*must(test.DoubleError(ctx, c, 2)).Err, "two")
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
