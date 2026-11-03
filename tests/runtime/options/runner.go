package runner

import (
	"context"
	"fmt"

	wrpc "wrpc.io/go"

	"driver/runner/test/options/to_test"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	must0(to_test.OptionNoneParam(ctx, c, nil))
	must0(to_test.OptionSomeParam(ctx, c, ptr("foo")))
	assert(must(to_test.OptionNoneResult(ctx, c)) == nil)
	assertEqual(*must(to_test.OptionSomeResult(ctx, c)), "foo")
	assertEqual(*must(to_test.OptionRoundtrip(ctx, c, ptr("foo"))), "foo")
	assertEqual(**must(to_test.DoubleOptionRoundtrip(ctx, c, ptr(ptr[uint32](42)))), 42)
	assert(*must(to_test.DoubleOptionRoundtrip(ctx, c, ptr[*uint32](nil))) == nil)
	assert(must(to_test.DoubleOptionRoundtrip(ctx, c, nil)) == nil)
	return nil
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
