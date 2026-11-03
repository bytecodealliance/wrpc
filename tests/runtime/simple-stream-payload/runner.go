//@ wasmtime-flags = '-Wcomponent-model-async'

package runner

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/runner/my/test/i"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	x := make(chan []uint8)
	go func() {
		// write one item
		x <- []uint8{0}
		// write two items
		x <- []uint8{1, 2}
		// write 1/2 items again
		x <- []uint8{3, 4}
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
