package runner

import (
	"context"
	"fmt"

	wrpc "wrpc.io/go"

	v1 "driver/runner/test/dep0_1_0/test"
	v2 "driver/runner/test/dep0_2_0/test"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	assertEqual(must(v1.X(ctx, c)), 1.0)
	assertEqual(must(v1.Y(ctx, c, 1.0)), 2.0)

	assertEqual(must(v2.X(ctx, c)), 2.0)
	assertEqual(must(v2.Z(ctx, c, 1.0, 1.0)), 4.0)
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
