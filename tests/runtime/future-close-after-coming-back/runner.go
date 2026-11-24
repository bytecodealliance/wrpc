//@ wasmtime-flags = '-Wcomponent-model-async'

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
	param := make(chan struct{}, 1)
	if _, _, err := the_test.F(ctx, c, param); err != nil {
		return err
	}
	param <- struct{}{}
	close(param)
	return nil
}
