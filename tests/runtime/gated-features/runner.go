//@ args = '--features y'

package runner

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/runner/foo/bar/bindings"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	if err := bindings.Y(ctx, c); err != nil {
		return err
	}
	return bindings.Z(ctx, c)
}
