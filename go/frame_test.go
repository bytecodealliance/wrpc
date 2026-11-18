package wrpc_test

import (
	"bytes"
	"slices"
	"testing"

	wrpc "wrpc.io/go"
)

func TestReadFrameHeaderBounds(t *testing.T) {
	for _, tc := range []struct {
		frame wrpc.Frame
		opts  []wrpc.ReadFrameHeaderOption
		err   string
	}{
		{frame: wrpc.Frame{Path: slices.Repeat([]uint32{7}, 64), Data: []byte("x")}},
		{frame: wrpc.Frame{Path: make([]uint32, 65)}, err: "path length of 65 exceeds maximum of 64"},
		{frame: wrpc.Frame{Path: []uint32{0, 0}}, opts: []wrpc.ReadFrameHeaderOption{wrpc.WithMaxFrameDepth(1)}, err: "path length of 2 exceeds maximum of 1"},
	} {
		var b bytes.Buffer
		if _, err := wrpc.WriteFrame(tc.frame, &b); err != nil {
			t.Fatal(err)
		}
		path, n, err := wrpc.ReadFrameHeader(&b, tc.opts...)
		if tc.err != "" {
			if err == nil || err.Error() != tc.err {
				t.Fatalf("expected error `%s`, got %v", tc.err, err)
			}
			continue
		}
		if err != nil {
			t.Fatal(err)
		}
		if !slices.Equal(path, tc.frame.Path) || int(n) != len(tc.frame.Data) || !bytes.Equal(b.Bytes(), tc.frame.Data) {
			t.Fatalf("unexpected frame header %v, %d", path, n)
		}
	}

	if _, _, err := wrpc.ReadFrameHeader(bytes.NewBufferString("\xff\xff\xff\xff\x0f")); err == nil || err.Error() != "path length of 4294967295 exceeds maximum of 64" {
		t.Fatalf("unexpected error reading a frame with an oversized path: %v", err)
	}
}
