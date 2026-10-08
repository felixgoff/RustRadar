// deck.gl 9.4's GlobeController pans like a trackball: it rotates the whole
// camera frame, so any drag that isn't perfectly straight (and wheel-zooming
// off-centre) also turns the bearing, and north drifts left or right. This
// controller pans along latitude and longitude instead and leaves the bearing
// to the explicit rotate gesture.
import { _GlobeController as GlobeController } from "@deck.gl/core";

const DEG = Math.PI / 180;
/** Keep clear of the poles, where longitude stops meaning anything. */
const MAX_LATITUDE = 85;

type PanFrame = { longitude: number; latitude: number; bearing: number };
type ViewProps = { longitude: number; latitude: number; zoom: number; bearing?: number };

// GlobeState isn't exported, so its methods are reached through the controller.
interface StateLike {
  getState(): {
    startPanPos?: [number, number] | null;
    startPanCameraFrame?: PanFrame | null;
    startPanAngularRate?: number | null;
  };
  getViewportProps(): ViewProps;
  _getUpdatedState(props: Partial<ViewProps>): StateLike;
}
type StateClass = new (options: object) => StateLike;

export class NorthUpGlobeController extends GlobeController {
  constructor(...args: ConstructorParameters<typeof GlobeController>) {
    super(...args);
    const self = this as unknown as { ControllerState: StateClass };
    const Base = self.ControllerState;
    const base = Base.prototype as unknown as {
      pan(this: StateLike, opts: { pos: [number, number]; startPos?: [number, number] }): StateLike;
      applyConstraints(props: ViewProps): ViewProps;
    };

    class NorthUpState extends Base {
      pan(opts: { pos: [number, number]; startPos?: [number, number] }): StateLike {
        const state = this.getState();
        const start = state.startPanPos ?? opts.startPos;
        const frame = state.startPanCameraFrame;
        const rate = state.startPanAngularRate; // radians of arc per pixel
        if (!start || !frame || !rate) return this;
        // the trackball's result only for its altitude-dependent zoom correction
        const { zoom } = base.pan.call(this, opts).getViewportProps();

        const right = (start[0] - opts.pos[0]) * rate;
        const up = (opts.pos[1] - start[1]) * rate;
        // screen axes to north and east, undoing the camera's bearing
        const b = frame.bearing * DEG;
        const north = up * Math.cos(b) - right * Math.sin(b);
        const east = up * Math.sin(b) + right * Math.cos(b);
        const latitude = Math.max(-MAX_LATITUDE, Math.min(MAX_LATITUDE, frame.latitude + north / DEG));
        const longitude = frame.longitude + east / DEG / Math.max(0.15, Math.cos(frame.latitude * DEG));
        return this._getUpdatedState({ longitude, latitude, bearing: frame.bearing, zoom });
      }

      applyConstraints(props: ViewProps): ViewProps {
        // Zooming toward the cursor rotates the frame too, so the bearing is
        // put back afterwards — but wrapped the way deck.gl wraps it. Handing
        // back an unwrapped bearing lets it climb past 180 gesture by gesture,
        // and "north up" then unwinds the long way round.
        const bearing = props.bearing;
        const result = base.applyConstraints.call(this, props);
        if (bearing !== undefined) result.bearing = (((bearing % 360) + 540) % 360) - 180;
        return result;
      }
    }
    self.ControllerState = NorthUpState;
  }
}
