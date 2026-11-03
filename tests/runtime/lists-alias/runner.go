package runner

import (
	"context"
	"fmt"

	wrpc "wrpc.io/go"

	"driver/runner/cat"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	if err := cat.Foo(ctx, c, []byte("hello")); err != nil {
		return err
	}
	t, err := cat.Bar(ctx, c)
	if err != nil {
		return err
	}
	if string(t) != "world" {
		panic(fmt.Sprintf("expected `world`; got `%s`", t))
	}
	return nil
}
