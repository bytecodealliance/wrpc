//go:generate $WIT_BINDGEN_WRPC go --world resources-client --out-dir bindings/resources_client --package wrpc.io/tests/go/bindings/resources_client ../wit

package integration_test

import (
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
	wrpc "wrpc.io/go"
	integration "wrpc.io/tests/go"
	"wrpc.io/tests/go/bindings/resources_client/strange"
	"wrpc.io/tests/go/bindings/resources_client/wrpc_test/integration/resources"
	"wrpc.io/tests/go/bindings/resources_server"
)

func TestResources(t *testing.T) {
	ctx, client := serve(t, func(s wrpc.Server) (func() error, error) {
		return resources_server.Serve(s, &integration.ResourcesHandler{}, integration.ResourcesStrangeHandler{})
	})

	foo, err := resources.NewFoo(ctx, client)
	require.NoError(t, err)

	v, err := resources.Foo_Bar(ctx, client, foo.Borrow())
	require.NoError(t, err)
	assert.Equal(t, "bar", v)

	v, err = resources.Bar(ctx, client, foo.Borrow())
	require.NoError(t, err)
	assert.Equal(t, "bar", v)

	n, err := strange.Bar(ctx, client, foo.Borrow())
	require.NoError(t, err)
	assert.Equal(t, uint64(42), n)

	v, err = resources.Foo_Foo(ctx, client, foo)
	require.NoError(t, err)
	assert.Equal(t, "foo", v)

	a, err := resources.NewCounter(ctx, client, 1)
	require.NoError(t, err)

	require.NoError(t, resources.Counter_IncrementBy(ctx, client, a.Borrow(), 2))

	n32, err := resources.Counter_GetCount(ctx, client, a.Borrow())
	require.NoError(t, err)
	assert.Equal(t, uint32(3), n32)

	b, err := resources.Counter_CloneCounter(ctx, client, a.Borrow())
	require.NoError(t, err)

	require.NoError(t, resources.Counter_IncrementBy(ctx, client, b.Borrow(), 1))

	n32, err = resources.Counter_Sum(ctx, client, a.Borrow(), b.Borrow())
	require.NoError(t, err)
	assert.Equal(t, uint32(7), n32)

	// TODO: Test dropping `foo`, also after moving it, once resource dropping is supported
}
