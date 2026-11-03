//@ wasmtime-flags = '-Wcomponent-model-async'

package runner

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/runner/a/b/the_test"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	param := make(chan struct{}, 1)
	if _, _, err := the_test.F(ctx, c, param); err != nil {
		return err
	}
	param <- struct{}{}
	close(param)
	return nil
}
