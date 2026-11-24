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
	x := make(chan uint32)
	go func() {
		x <- 0
		close(x)
	}()
	if err := i.CancelBeforeRead(ctx, c, x); err != nil {
		return err
	}

	y := make(chan uint32)
	go func() {
		y <- 0
		close(y)
	}()
	if err := i.CancelAfterRead(ctx, c, y); err != nil {
		return err
	}

	data := make(chan uint32)
	signal := make(chan struct{})
	go func() {
		signal <- struct{}{}
		close(signal)
		data <- 4
		close(data)
	}()
	if err := i.StartReadThenCancel(ctx, c, data, signal); err != nil {
		return err
	}
	return nil
}
