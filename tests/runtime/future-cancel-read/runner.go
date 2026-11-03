//@ wasmtime-flags = '-Wcomponent-model-async'

package runner

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/runner/my/test/i"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	x := make(chan uint32)
	go func() {
		x <- 0
		close(x)
	}()
	if _, err := i.CancelBeforeRead(ctx, c, x); err != nil {
		return err
	}

	x = make(chan uint32)
	go func() {
		x <- 0
		close(x)
	}()
	if _, err := i.CancelAfterRead(ctx, c, x); err != nil {
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
	errs, err := i.StartReadThenCancel(ctx, c, data, signal)
	if err != nil {
		return err
	}
	for err := range errs {
		return err
	}
	return nil
}
