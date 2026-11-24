//@ wasmtime-flags = '-Wcomponent-model-async'

package runner

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/runner/my/test/i"
)

func Run(ctx context.Context, connect func() (*wrpc.Framed, error)) error {
	c, err := connect()
	if err != nil {
		return err
	}
	x := make(chan struct{})
	go func() {
		x <- struct{}{}
		close(x)
	}()
	if err := i.ReadFuture(ctx, c, x); err != nil {
		return err
	}

	y := make(chan struct{})
	go func() {
		y <- struct{}{}
		close(y)
	}()
	err = i.DropFuture(ctx, c, y)
	return err
}
