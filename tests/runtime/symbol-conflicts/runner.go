package runner

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/runner/my/inline/bar1"
	"driver/runner/my/inline/bar2"
	"driver/runner/my/inline/foo1"
	"driver/runner/my/inline/foo2"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	if err := foo1.Foo(ctx, c); err != nil {
		return err
	}
	if err := foo2.Foo(ctx, c); err != nil {
		return err
	}
	if _, err := bar1.Bar(ctx, c); err != nil {
		return err
	}
	_, err := bar2.Bar(ctx, c)
	return err
}
