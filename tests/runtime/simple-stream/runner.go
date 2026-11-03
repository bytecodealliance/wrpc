//@ wasmtime-flags = '-Wcomponent-model-async'

package runner

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/runner/my/test/i"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	x := make(chan []struct{})
	go func() {
		// write one item
		x <- []struct{}{{}}
		// write two items
		x <- []struct{}{{}, {}}
		close(x)
	}()
	errs, err := i.ReadStream(ctx, c, x)
	if err != nil {
		return err
	}
	for err := range errs {
		return err
	}
	return nil
}
