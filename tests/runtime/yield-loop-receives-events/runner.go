//@ wasmtime-flags = '-Wcomponent-model-async'

package runner

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/runner/test/common/i_runner"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	return i_runner.F(ctx, c)
}
