//@ wasmtime-flags = '-Wcomponent-model-async'

package runner

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/runner/a/b/the_test"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	param := make(chan struct{}, 1)
	param <- struct{}{}
	close(param)
	errs, err := the_test.F(ctx, c, param)
	if err != nil {
		return err
	}
	for err := range errs {
		return err
	}
	return nil
}
