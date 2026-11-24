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
	x := make(chan string, 1)
	x <- "hello"
	close(x)
	if err := i.TakeThenDrop(ctx, c, x); err != nil {
		return err
	}

	x = make(chan string, 1)
	x <- "hello3"
	close(x)
	if err := i.ReadAndDrop(ctx, c, x); err != nil {
		return err
	}
	return nil
}
