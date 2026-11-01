//go:generate $WIT_BINDGEN_WRPC go --gofmt=false --world types --out-dir bindings/types --package wrpc.io/tests/go/bindings/types ../wit

package integration_test

import (
	"bytes"
	"context"
	"encoding/binary"
	"math"
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
	wrpc "wrpc.io/go"
	"wrpc.io/tests/go/bindings/types/wrpc_test/integration/get_types"
)

type indexReader struct {
	*bytes.Buffer
}

func (r *indexReader) Index(path ...uint32) (wrpc.IndexReadCloser, error) {
	panic("not implemented")
}

func (r *indexReader) Buffered() int {
	return r.Len()
}

func TestFlags(t *testing.T) {
	flags := get_types.FeatureFlags{ShowA: true, ShowC: true}
	var b bytes.Buffer
	_, err := flags.WriteToIndex(&b)
	require.NoError(t, err)
	assert.Equal(t, []byte{0b101}, b.Bytes())

	var got get_types.FeatureFlags
	require.NoError(t, got.ReadFromIndex(&indexReader{&b}))
	assert.Equal(t, flags, got)

	got = get_types.FeatureFlags{}
	assert.EqualError(t, got.ReadFromIndex(&indexReader{bytes.NewBuffer([]byte{0x80})}), "bit not associated with any flag is set")
	assert.Zero(t, got)
}

func TestCanonicalNaN(t *testing.T) {
	nan := []byte("\x00\x00\xc0\x7f" + "\x00\x00\x00\x00\x00\x00\xf8\x7f")
	for _, bits := range [][2]uint64{
		{0x7f80_0001, 0x7ff0_0000_0000_0001},
		{0x7fa0_0000, 0x7ff4_0000_0000_0000},
		{0x7fc0_0001, 0x7ff8_0000_0000_0001},
		{0x7fff_ffff, 0x7fff_ffff_ffff_ffff},
		{0xff80_0001, 0xfff0_0000_0000_0001},
		{0xffc0_0000, 0xfff8_0000_0000_0000},
		{0xffff_ffff, 0xffff_ffff_ffff_ffff},
	} {
		v := get_types.Floats{A: math.Float32frombits(uint32(bits[0])), B: math.Float64frombits(bits[1])}
		var b bytes.Buffer
		_, err := v.WriteToIndex(&b)
		require.NoError(t, err)
		assert.Equal(t, nan, b.Bytes(), "%#x", bits)
		assert.Equal(t, nan, wrpc.AppendFloat64(wrpc.AppendFloat32(nil, v.A), v.B), "%#x", bits)
	}

	for _, v := range []get_types.Floats{
		{A: 0, B: 0},
		{A: float32(math.Copysign(0, -1)), B: math.Copysign(0, -1)},
		{A: 1.5, B: -1.5},
		{A: math.SmallestNonzeroFloat32, B: math.SmallestNonzeroFloat64},
		{A: math.MaxFloat32, B: math.MaxFloat64},
		{A: float32(math.Inf(1)), B: math.Inf(-1)},
	} {
		var b bytes.Buffer
		_, err := v.WriteToIndex(&b)
		require.NoError(t, err)
		assert.Equal(t, binary.LittleEndian.AppendUint64(binary.LittleEndian.AppendUint32(nil, math.Float32bits(v.A)), math.Float64bits(v.B)), b.Bytes(), "%v", v)
	}
}

func TestSignedLEB128(t *testing.T) {
	for _, tc := range []struct {
		data string
		want [3]int64
		err  string
	}{
		{data: "\x80\x80\x7e" + "\x80\x80\x80\x80\x78" + "\x80\x80\x80\x80\x80\x80\x80\x80\x80\x7f", want: [3]int64{math.MinInt16, math.MinInt32, math.MinInt64}},
		{data: "\xff\xff\x01" + "\xff\xff\xff\xff\x07" + "\xff\xff\xff\xff\xff\xff\xff\xff\xff\x00", want: [3]int64{math.MaxInt16, math.MaxInt32, math.MaxInt64}},
		{data: "\x80\x80\x7a" + "\x00" + "\x00", err: "s16 overflows"},
		{data: "\x00" + "\x80\x80\x80\x80\x08" + "\x00", err: "s32 overflows"},
		{data: "\x00" + "\x00" + "\x80\x80\x80\x80\x80\x80\x80\x80\x80\x01", err: "s64 overflows"},
	} {
		ctx, client := serve(t, func(s wrpc.Server) (func() error, error) {
			return s.Serve("wrpc-test:integration/get-types", "get-signed", func(ctx context.Context, w wrpc.IndexWriteCloser, r wrpc.IndexReadCloser) {
				_, _ = w.Write([]byte(tc.data))
				_ = w.Close()
			})
		})
		a, b, c, err := get_types.GetSigned(ctx, client)
		if tc.err != "" {
			assert.ErrorContains(t, err, tc.err, "%q", tc.data)
			continue
		}
		require.NoError(t, err)
		assert.Equal(t, tc.want, [3]int64{int64(a), int64(b), c})
	}
}
