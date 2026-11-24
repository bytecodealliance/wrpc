//@ args = '--generate-unused-types'

package runner

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/runner/foo/bar/component"
)

var (
	_ component.UnusedEnum
	_ component.UnusedRecord
	_ component.UnusedVariant
)

func Run(ctx context.Context, connect func() (*wrpc.Framed, error)) error {
	c, err := connect()
	if err != nil {
		return err
	}
	return component.Foo(ctx, c)
}
