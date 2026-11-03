//@ wasmtime-flags = '-Wcomponent-model-async'

package runner

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/runner/a/b/i"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	return i.F(ctx, c)
}
