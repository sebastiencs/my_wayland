precision highp float;
varying vec2 v_local_position;
uniform vec4 u_outer_rect;
uniform vec4 u_inner_rect;
uniform float u_outer_corner_radius;
uniform float u_outer_rounding_power;
uniform float u_inner_corner_radius;
uniform float u_inner_rounding_power;
uniform vec2 u_item_size;
uniform vec4 u_gradient_start;
uniform vec4 u_gradient_end;
uniform float u_gradient_angle_degrees;
uniform float u_alpha;
bool pointInRect(vec2 point, vec4 rect) {
    return point.x >= rect.x
        && point.x < (rect.x + rect.z)
        && point.y >= rect.y
        && point.y < (rect.y + rect.w);
}
float roundedCornerDistance(vec2 delta, float roundingPower) {
    vec2 absoluteDelta = abs(delta);
    return pow(pow(absoluteDelta.x, roundingPower) + pow(absoluteDelta.y, roundingPower), 1.0 / roundingPower);
}
void maybeDiscardRoundedCorner(vec2 point, vec2 center, float radius, float roundingPower) {
    vec2 delta = point - center;
    if (roundedCornerDistance(delta, roundingPower) > radius) {
        discard;
    }
}
bool pointInRoundedRect(vec2 point, vec4 rect, float radius, float roundingPower) {
    if (!pointInRect(point, rect)) {
        return false;
    }
    if (radius <= 0.0) {
        return true;
    }
    float left = rect.x + radius;
    float top = rect.y + radius;
    float right = rect.x + rect.z - radius;
    float bottom = rect.y + rect.w - radius;
    if (point.x < left && point.y < top) {
        return roundedCornerDistance(point - vec2(left, top), roundingPower) <= radius;
    }
    if (point.x >= right && point.y < top) {
        return roundedCornerDistance(point - vec2(right, top), roundingPower) <= radius;
    }
    if (point.x < left && point.y >= bottom) {
        return roundedCornerDistance(point - vec2(left, bottom), roundingPower) <= radius;
    }
    if (point.x >= right && point.y >= bottom) {
        return roundedCornerDistance(point - vec2(right, bottom), roundingPower) <= radius;
    }
    return true;
}
void discardOutsideRoundedRect(vec2 point, vec4 rect, float radius, float roundingPower) {
    if (!pointInRoundedRect(point, rect, radius, roundingPower)) {
        discard;
    }
}
void main() {
    float outerRadius = min(u_outer_corner_radius, min(u_outer_rect.z, u_outer_rect.w) * 0.5);
    float outerRoundingPower = max(u_outer_rounding_power, 0.0001);
    discardOutsideRoundedRect(v_local_position, u_outer_rect, outerRadius, outerRoundingPower);
    float innerRadius = min(u_inner_corner_radius, min(u_inner_rect.z, u_inner_rect.w) * 0.5);
    float innerRoundingPower = max(u_inner_rounding_power, 0.0001);
    if (pointInRoundedRect(v_local_position, u_inner_rect, innerRadius, innerRoundingPower)) {
        discard;
    }
    vec2 normalized = (v_local_position / max(u_item_size, vec2(1.0, 1.0))) - vec2(0.5, 0.5);
    float angle = radians(u_gradient_angle_degrees);
    vec2 direction = vec2(cos(angle), sin(angle));
    float progress = clamp(dot(normalized, direction) + 0.5, 0.0, 1.0);
    gl_FragColor = mix(u_gradient_start, u_gradient_end, progress) * u_alpha;
}
