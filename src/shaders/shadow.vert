attribute vec2 a_position;
uniform mat3 u_transform;
uniform vec2 u_item_size;
varying vec2 v_local_position;
void main() {
    vec3 position = u_transform * vec3(a_position, 1.0);
    gl_Position = vec4(position.xy, 0.0, 1.0);
    v_local_position = a_position * u_item_size;
}
