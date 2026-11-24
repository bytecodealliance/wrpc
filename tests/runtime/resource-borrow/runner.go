package runner

import (
	"context"
	"fmt"

	wrpc "wrpc.io/go"

	"driver/runner/test/resource_borrow/to_test"
)

func Run(ctx context.Context, connect func() (*wrpc.Framed, error)) error {
	c, err := connect()
	if err != nil {
		return err
	}
	thing := must(to_test.NewThing(ctx, c, 254))
	assertEqual(must(to_test.Foo(ctx, c, thing.Borrow())), 254+1+2)
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
