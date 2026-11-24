package runner

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/runner/a/b/the_test"
)

func Run(ctx context.Context, connect func() (*wrpc.Framed, error)) error {
	c, err := connect()
	if err != nil {
		return err
	}
	return the_test.X(ctx, c)
}
