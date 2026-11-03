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

func Run(ctx context.Context, c wrpc.Invoker) error {
	return component.Foo(ctx, c)
}
