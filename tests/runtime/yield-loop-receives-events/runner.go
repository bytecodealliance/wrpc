//@ wasmtime-flags = '-Wcomponent-model-async'

package runner

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/runner/test/common/i_runner"
)

func Run(ctx context.Context, connect func() (*wrpc.Framed, error)) error {
	c, err := connect()
	if err != nil {
		return err
	}
	return i_runner.F(ctx, c)
}
