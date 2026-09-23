package fsdfeed

import "testing"

func TestParseDatafeedPreservesControllerAndObserverAssignments(t *testing.T) {
	snap, err := ParseDatafeed([]byte(`{"controllers":[{"cid":"1000","callsign":"ZSPD_TWR","facility":4,"frequency":"118.500"},{"cid":"2000","callsign":"OBS01","facility":0,"frequency":"122.800"}]}`))
	if err != nil {
		t.Fatal(err)
	}
	controller := snap.ByCallsign["ZSPD_TWR"]
	if controller.Facility != 4 || controller.FrequencyKHz != 118500 {
		t.Fatalf("controller assignment = %+v", controller)
	}
	observer := snap.ByCallsign["OBS01"]
	if observer.Facility != 0 || observer.FrequencyKHz != 122800 || !observer.IsObserver || observer.RadiusNM > 100 {
		t.Fatalf("observer assignment = %+v", observer)
	}
}
