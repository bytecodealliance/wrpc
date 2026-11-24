//@ args = '--features y'

package runner

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/runner/foo/bar/bindings"
)

func Run(ctx context.Context, connect func() (*wrpc.Framed, error)) error {
	c, err := connect()
	if err != nil {
		return err
	}
	if err := bindings.Y(ctx, c); err != nil {
		return err
	}
	return bindings.Z(ctx, c)
}
