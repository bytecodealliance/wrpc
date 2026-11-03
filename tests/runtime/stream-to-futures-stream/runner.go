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
		x <- []uint8{10, 20, 30}
		// Close the writer so the reader sees the end of the stream.
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
