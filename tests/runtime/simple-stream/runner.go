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
	x := make(chan []struct{})
	go func() {
		// write one item
		x <- []struct{}{{}}
		// write two items
		x <- []struct{}{{}, {}}
		close(x)
	}()
	if err := i.ReadStream(ctx, c, x); err != nil {
		return err
	}
	return nil
}
