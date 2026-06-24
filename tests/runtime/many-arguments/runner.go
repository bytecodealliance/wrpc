package runner

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/runner/test/many_arguments/to_test"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	return to_test.ManyArguments(ctx, c, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16)
}
