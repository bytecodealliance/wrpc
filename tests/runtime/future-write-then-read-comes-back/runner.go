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
	param := make(chan struct{})
	go func() {
		param <- struct{}{}
		close(param)
	}()
	rx, inv, err := the_test.F(ctx, c, param)
	if err != nil {
		return err
	}
	done := make(chan error, 1)
	go func() { done <- inv.Wait() }()
	<-rx
	return <-done
}
