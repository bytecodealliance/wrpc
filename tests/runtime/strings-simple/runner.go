package runner

import (
	"context"
	"fmt"

	wrpc "wrpc.io/go"

	"driver/runner/cat"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	if err := cat.Foo(ctx, c, "hello"); err != nil {
		return err
	}
	value, err := cat.Bar(ctx, c)
	if err != nil {
		return err
	}
	if value != "world" {
		panic(fmt.Sprintf("expected `world`; got `%v`", value))
	}
	return nil
}
