//@ wasmtime-flags = '-Wcomponent-model-async'

package runner

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/runner/a/b/the_test"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	param := make(chan struct{})
	go func() {
		param <- struct{}{}
		close(param)
	}()
	if _, err := the_test.Set(ctx, c, param); err != nil {
		return err
	}
	_, _, err := the_test.Get(ctx, c)
	return err
}
