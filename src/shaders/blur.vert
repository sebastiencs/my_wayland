attribute vec2 a_position;
attribute vec2 a_texcoord;
uniform mat3 u_transform;
uniform mat3 u_tex_transform;
uniform vec2 u_item_size;
varying vec2 v_texcoord;
void main() {
    vec3 position = u_transform * vec3(a_position, 1.0);
    vec3 texcoord = u_tex_transform * vec3(a_texcoord, 1.0);
    gl_Position = vec4(position.xy, 0.0, 1.0);
    v_texcoord = texcoord.xy;
}
