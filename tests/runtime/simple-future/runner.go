//@ wasmtime-flags = '-Wcomponent-model-async'

package runner

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/runner/my/test/i"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	x := make(chan struct{})
	go func() {
		x <- struct{}{}
		close(x)
	}()
	errs, err := i.ReadFuture(ctx, c, x)
	if err != nil {
		return err
	}
	for err := range errs {
		return err
	}

	x = make(chan struct{})
	go func() {
		x <- struct{}{}
		close(x)
	}()
	_, err = i.DropFuture(ctx, c, x)
	return err
}
