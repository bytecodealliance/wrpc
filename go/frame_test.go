package wrpc_test

import (
	"bytes"
	"slices"
	"testing"

	wrpc "wrpc.io/go"
)

func TestReadFrameBounds(t *testing.T) {
	for _, tc := range []struct {
		frame wrpc.Frame
		opts  []wrpc.ReadFrameOption
		err   string
	}{
		{frame: wrpc.Frame{Path: slices.Repeat([]uint32{7}, 64), Data: []byte("x")}},
		{frame: wrpc.Frame{Path: make([]uint32, 65)}, err: "path length of 65 exceeds maximum of 64"},
		{frame: wrpc.Frame{Data: make([]byte, 16<<20+1)}, err: "data length of 16777217 exceeds maximum of 16777216"},
		{frame: wrpc.Frame{Path: []uint32{0, 0}}, opts: []wrpc.ReadFrameOption{wrpc.WithMaxFrameDepth(1)}, err: "path length of 2 exceeds maximum of 1"},
		{frame: wrpc.Frame{Path: []uint32{0}, Data: []byte("hi")}, opts: []wrpc.ReadFrameOption{wrpc.WithMaxFrameSize(1)}, err: "data length of 2 exceeds maximum of 1"},
	} {
		var b bytes.Buffer
		if _, err := wrpc.WriteFrame(tc.frame, &b); err != nil {
			t.Fatal(err)
		}
		frame, err := wrpc.ReadFrame(&b, tc.opts...)
		if tc.err != "" {
			if err == nil || err.Error() != tc.err {
				t.Fatalf("expected error `%s`, got %v", tc.err, err)
			}
			continue
		}
		if err != nil {
			t.Fatal(err)
		}
		if !slices.Equal(frame.Path, tc.frame.Path) || !bytes.Equal(frame.Data, tc.frame.Data) {
			t.Fatalf("unexpected frame %v", frame)
		}
	}

	if _, err := wrpc.ReadFrame(bytes.NewBufferString("\xff\xff\xff\xff\x0f")); err == nil || err.Error() != "path length of 4294967295 exceeds maximum of 64" {
		t.Fatalf("unexpected error reading a frame with an oversized path: %v", err)
	}
}
