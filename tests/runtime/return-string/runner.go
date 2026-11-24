//@ wasmtime-flags = '-Wcomponent-model-async'

package runner

import (
	"context"
	"fmt"
	"runtime"

	wrpc "wrpc.io/go"

	"driver/runner/my/test/i"
)

func Run(ctx context.Context, connect func() (*wrpc.Framed, error)) error {
	c, err := connect()
	if err != nil {
		return err
	}
	got, err := i.ReturnString(ctx, c)
	if err != nil {
		return err
	}
	if got != "hello" {
		panic(fmt.Sprintf("expected \"hello\", got %q", got))
	}

	runtime.GC()
	return nil
}
