package runner

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/runner/my/inline/foo"
)

func Run(ctx context.Context, connect func() (*wrpc.Framed, error)) error {
	c, err := connect()
	if err != nil {
		return err
	}
	_, err = foo.NewBar(ctx, c)
	return err
}
