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
		x <- []uint8{10, 20, 30}
		// Close the writer so the reader sees the end of the stream.
		close(x)
	}()
	if err := i.ReadStream(ctx, c, x); err != nil {
		return err
	}
	return nil
}
