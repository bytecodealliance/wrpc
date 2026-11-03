package runner

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/runner/test/resource_alias/e1"
	"driver/runner/test/resource_alias/e2"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	x := must(e1.NewX(ctx, c, 42))
	if _, err := e1.A(ctx, c, &e1.Foo{X: x}); err != nil {
		return err
	}

	x = must(e1.NewX(ctx, c, 7))
	fooE2 := &e2.Foo{X: x}
	x = must(e1.NewX(ctx, c, 8))
	barE2 := &e1.Foo{X: x}
	y := must(e1.NewX(ctx, c, 8))
	_, err := e2.A(ctx, c, fooE2, barE2, y.Borrow())
	return err
}

func must[T any](v T, err error) T {
	if err != nil {
		panic(err)
	}
	return v
}
