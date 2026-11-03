package runner

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/runner/a/b/the_test"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	return the_test.X(ctx, c)
}
