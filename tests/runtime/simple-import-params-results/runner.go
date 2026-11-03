//@ wasmtime-flags = '-Wcomponent-model-async'

package runner

import (
	"context"
	"fmt"

	wrpc "wrpc.io/go"

	"driver/runner/a/b/i"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	if err := i.OneArgument(ctx, c, 1); err != nil {
		return err
	}
	assertEqual(must(i.OneResult(ctx, c)), 2)
	assertEqual(must(i.OneArgumentAndResult(ctx, c, 3)), 4)
	if err := i.TwoArguments(ctx, c, 5, 6); err != nil {
		return err
	}
	assertEqual(must(i.TwoArgumentsAndResult(ctx, c, 7, 8)), 9)
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
