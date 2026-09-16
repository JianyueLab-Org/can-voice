package control

import (
	"encoding/json"
	"os"
	"reflect"
	"testing"
)

// 控制面消息的跨实现黄金文件。Rust 侧读同一份。
//
// 数据面的包头早就有 testdata/wire-golden.json 两边一起测；控制面此前没有对应物,
// 于是 Go 与 Rust 是两份完全独立的实现,只靠 e2e 走通的那几条消息间接覆盖。
// SUBACK 的 rejected_xc、NOTICE 这类少走的路径上,字段改名会静默漂:一边改了名,
// 另一边解出来是默认值,两边都不报错,而线上表现是"这个字段永远是空的"。
//
// 比较规则见黄金文件自己的 how/asymmetries 两段:**wire 里每一个非 null 的键,
// 都必须原样出现在重新编码的结果里**。不按字节比,也不整体比相等——那会把
// Go 的 nil-slice-编成-null 和 Rust 多出的零值键误判成故障。
func TestControlGolden(t *testing.T) {
	raw, err := os.ReadFile("../../testdata/control-golden.json")
	if err != nil {
		t.Fatalf("控制面黄金文件读不到: %v", err)
	}
	var golden struct {
		Cases []struct {
			Name string          `json:"name"`
			Wire json.RawMessage `json:"wire"`
		} `json:"cases"`
	}
	if err := json.Unmarshal(raw, &golden); err != nil {
		t.Fatalf("控制面黄金文件不是合法 JSON: %v", err)
	}
	// 下界:一份被清空的黄金文件不该静默通过。wire-golden 那边同理。
	if len(golden.Cases) < 12 {
		t.Fatalf("黄金文件只剩 %d 条用例,少于下界 12——是不是被删过?", len(golden.Cases))
	}

	seen := map[string]bool{}
	for _, c := range golden.Cases {
		t.Run(c.Name, func(t *testing.T) {
			m, err := Decode(c.Wire)
			if err != nil {
				t.Fatalf("解不开这条线上样例: %v", err)
			}
			got, err := Encode(m)
			if err != nil {
				t.Fatalf("解开了却编不回去: %v", err)
			}
			var want, have map[string]any
			if err := json.Unmarshal(c.Wire, &want); err != nil {
				t.Fatalf("用例的 wire 不是一个 JSON 对象: %v", err)
			}
			if err := json.Unmarshal(got, &have); err != nil {
				t.Fatalf("编出来的不是一个 JSON 对象: %v", err)
			}
			seen[want["type"].(string)] = true
			for k, w := range want {
				if w == nil {
					// null 不比:Go 的 nil slice 编成 null,Rust 编成 []。
					continue
				}
				h, ok := have[k]
				if !ok {
					t.Errorf("键 %q 在重新编码之后消失了——改名了?", k)
					continue
				}
				if !reflect.DeepEqual(w, h) {
					t.Errorf("键 %q 变了: wire 是 %v,重新编码是 %v", k, w, h)
				}
			}
		})
	}

	// 八个类型一个都不能漏。漏掉的那个就是将来会悄悄漂的那个。
	for _, ty := range []string{"HELLO", "READY", "SUB", "SUBACK", "NOTICE", "PING", "PONG", "BYE"} {
		if !seen[ty] {
			t.Errorf("黄金文件里没有 %s 的用例", ty)
		}
	}
}
