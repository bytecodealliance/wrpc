package runner

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/runner/my/inline/foo"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	_, err := foo.NewBar(ctx, c)
	return err
}
