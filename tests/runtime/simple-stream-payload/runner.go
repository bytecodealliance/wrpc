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
	if err := i.ReadStream(ctx, c, x); err != nil {
		return err
	}
	return nil
}
